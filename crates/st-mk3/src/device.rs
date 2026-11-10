//! The real controller: its HID interface through `async-hid` and its display interface through
//! `nusb`. Both crates are pure Rust and need no system library (IOKit on macOS, hidraw and usbfs
//! on Linux, the Win32 HID and WinUSB interfaces on Windows).
//!
//! **Nothing in this file has run against hardware.** It type-checks for Linux and macOS; what it
//! assumes is listed in the crate documentation.
//!
//! Access rights: on Linux the user needs read and write access to the controller's `hidraw` node
//! and, for the screens, to its USB device node (a udev rule for `17cc:1600`). On macOS reading
//! input needs the Input Monitoring permission.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use async_hid::{AsyncHidRead, AsyncHidWrite, DeviceReader, DeviceWriter, HidBackend, HidError};
use futures_lite::StreamExt;
use nusb::transfer::{Buffer, Bulk, Out};
use nusb::MaybeFuture;

use crate::protocol::{PRODUCT_ID, VENDOR_ID};
use crate::transport::{HidConnection, HidLoss, HidReader, HidTransport, HidWriter, ScreenPipe, ScreenTransport};

/// The display interface's number and its bulk OUT endpoint.
const SCREEN_INTERFACE: u8 = 5;
const SCREEN_ENDPOINT: u8 = 0x04;
/// How long one LED report may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs a future on this thread until it finishes or `deadline` passes. On a timeout the future is
/// dropped, which cancels it.
fn block_on_until<F: Future>(future: F, deadline: Instant) -> Option<F::Output> {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return Some(output);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        thread::park_timeout(remaining);
    }
}

/// The controller's HID interface on this system.
#[derive(Default)]
pub struct SystemHid {
    backend: HidBackend,
}

impl SystemHid {
    /// The system's HID devices.
    pub fn new() -> Self {
        Self::default()
    }
}

struct SystemHidReader(DeviceReader);
struct SystemHidWriter(DeviceWriter);

impl HidTransport for SystemHid {
    fn open(&self) -> Result<Option<HidConnection>, String> {
        let limit = Instant::now() + Duration::from_secs(5);
        let found = block_on_until(
            async {
                let mut devices = self.backend.enumerate().await?;
                while let Some(device) = devices.next().await {
                    if device.vendor_id == VENDOR_ID && device.product_id == PRODUCT_ID {
                        return Ok(Some(device));
                    }
                }
                Ok::<_, HidError>(None)
            },
            limit,
        )
        .ok_or("listing HID devices took too long")?
        .map_err(|error| error.to_string())?;
        let Some(device) = found else { return Ok(None) };
        let (reader, writer) = block_on_until(device.open(), limit).ok_or("opening the controller took too long")?.map_err(|error| error.to_string())?;
        Ok(Some(HidConnection { reader: Box::new(SystemHidReader(reader)), writer: Box::new(SystemHidWriter(writer)) }))
    }
}

impl HidReader for SystemHidReader {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidLoss> {
        // Room for a double-pumped pad delivery and more.
        let mut buffer = [0u8; 512];
        match block_on_until(self.0.read_input_report(&mut buffer), Instant::now() + timeout) {
            None => Ok(None),
            Some(Ok(length)) => Ok(Some(buffer[..length.min(buffer.len())].to_vec())),
            Some(Err(HidError::Disconnected | HidError::NotConnected)) => Err(HidLoss::Disconnected),
            Some(Err(error)) => Err(HidLoss::Failed(error.to_string())),
        }
    }
}

impl HidWriter for SystemHidWriter {
    fn write(&mut self, report: &[u8]) -> Result<(), String> {
        match block_on_until(self.0.write_output_report(report), Instant::now() + WRITE_TIMEOUT) {
            None => Err("the controller did not take the report in time".into()),
            Some(result) => result.map_err(|error| error.to_string()),
        }
    }
}

/// The controller's display interface on this system.
#[derive(Default)]
pub struct SystemScreens;

impl SystemScreens {
    /// The system's USB devices.
    pub fn new() -> Self {
        SystemScreens
    }

    fn find() -> Result<Option<nusb::DeviceInfo>, String> {
        let mut devices = nusb::list_devices().wait().map_err(|error| format!("USB devices could not be listed: {error}"))?;
        Ok(devices.find(|device| device.vendor_id() == VENDOR_ID && device.product_id() == PRODUCT_ID))
    }
}

struct SystemPipe {
    endpoint: nusb::Endpoint<Bulk, Out>,
    // Dropped after the endpoint: releases the claim.
    _interface: nusb::Interface,
}

impl ScreenTransport for SystemScreens {
    fn is_present(&self) -> bool {
        matches!(Self::find(), Ok(Some(_)))
    }

    fn open(&self) -> Result<Box<dyn ScreenPipe>, String> {
        let device = Self::find()?.ok_or("Maschine MK3 screen interface not found.")?;
        let device = device.open().wait().map_err(|error| format!("the controller's USB device could not be opened: {error}"))?;
        // Fails while other software (the vendor's agents) holds the interface.
        let interface = device.claim_interface(SCREEN_INTERFACE).wait().map_err(|error| format!("the display interface is in use: {error}"))?;
        let endpoint = interface.endpoint::<Bulk, Out>(SCREEN_ENDPOINT).map_err(|error| format!("the display endpoint is unavailable: {error}"))?;
        Ok(Box::new(SystemPipe { endpoint, _interface: interface }))
    }
}

impl ScreenPipe for SystemPipe {
    fn write(&mut self, packet: &[u8], timeout: Duration) -> Result<usize, String> {
        let completion = self.endpoint.transfer_blocking(Buffer::from(packet.to_vec()), timeout);
        completion.status.map_err(|error| error.to_string())?;
        Ok(completion.actual_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn futures_are_driven_to_completion_or_dropped_at_the_deadline() {
        assert_eq!(block_on_until(async { 7 }, Instant::now()), Some(7), "a ready future needs no time");
        let started = Instant::now();
        assert_eq!(block_on_until(std::future::pending::<()>(), started + Duration::from_millis(30)), None);
        assert!(started.elapsed() >= Duration::from_millis(30) && started.elapsed() < Duration::from_secs(2));
        // Woken from another thread.
        let (sender, receiver) = std::sync::mpsc::channel::<Waker>();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (for_future, for_thread) = (flag.clone(), flag.clone());
        thread::spawn(move || {
            let waker = receiver.recv().unwrap();
            thread::sleep(Duration::from_millis(20));
            for_thread.store(true, std::sync::atomic::Ordering::SeqCst);
            waker.wake();
        });
        let mut sent = false;
        let future = std::future::poll_fn(move |context| {
            if !sent {
                sent = true;
                sender.send(context.waker().clone()).unwrap();
            }
            if for_future.load(std::sync::atomic::Ordering::SeqCst) {
                Poll::Ready("woken")
            } else {
                Poll::Pending
            }
        });
        assert_eq!(block_on_until(future, Instant::now() + Duration::from_secs(5)), Some("woken"));
    }

    /// No controller is attached to the machines that run the tests: looking for one must report
    /// that, or an explained failure, and never hang or panic.
    #[test]
    fn an_absent_controller_is_reported_as_absent() {
        if let Ok(found) = SystemHid::new().open() {
            assert!(found.is_none(), "no Maschine MK3 is expected on a test machine");
        }
        let screens = SystemScreens::new();
        if !screens.is_present() {
            assert!(screens.open().is_err());
        }
    }
}
