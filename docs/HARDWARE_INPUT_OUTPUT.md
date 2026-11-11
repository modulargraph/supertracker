# Hardware inputs, sampling and MIDI output

Open **Line inputs…** above the pattern grid (also available in the command palette), or
use **Live input** on the selected track in the mixer. Assign a mono channel or stereo
pair to each track, enable the inputs you want to hear, and apply. Several tracks can use
different channels of the same audio interface at once. Each input enters its track ahead
of the inserts: it passes through that track's inserts, fader, sends and meters, and is
part of what is recorded from the master. Live inputs are heard without pressing Play.
**Monitor** off keeps a route but leaves it out of the track (for listening through the
interface instead). Existing samples and instrument plug-ins remain on their tracks.
A route is part of the project: changing it is an ordinary edit with Undo, and a saved
project brings its routes back.

Select the interface in Audio & MIDI settings; the choice is remembered. To combine
separate physical interfaces, create a macOS Aggregate Device in Audio MIDI Setup and
select it as the input device (routes are then assigned on the aggregate). Saved routes
remember their interface by the name the system lists it under: a route whose interface
is not the selected input is paused, with the reason shown, and never connected to a
different source. It runs again when its interface is the input. The Line inputs dialog
shows paused routes and lets you explicitly assign the current interface. The input
device is opened when a route needs it, which includes opening a project that has
enabled routes; macOS asks for microphone access the first time.

**Hardware return latency.** A hardware instrument that follows the song answers late:
its note leaves when the block is heard, and its audio comes back through the input. A
track with a live input therefore has latency in the mixer's delay compensation (one
output block, the input latency the device reports, and the delay of the input's way
into the mixer), and the other tracks wait for it, so the return lines up with them.

**One interface or two.** Input and output of one interface share a clock: the input's
frames go into the mixer as they are, one block late. An input on another device than
the output (a USB microphone with the built-in output, one interface in and another
out) has a clock of its own. No two clocks agree: they differ by tens to hundreds of
parts per million and wander with temperature, and the input may run at another sample
rate altogether (a 44.1 kHz microphone in a 48 kHz project; the input is opened at the
project's rate when it supports it and at its nearest rate otherwise). Such an input is
resampled onto the output's clock continuously: the host measures the ratio between the
two clocks from the times of their callbacks and follows it slowly enough that the
pitch does not move audibly. There is no gap and no skip while both devices run, a
take stays in sync with the song over its whole length, and the input can be used for
Freeze by recording. The price is latency: such an input waits one output block, one
input block, 44 frames of the conversion filter and a margin of 1 to 5 ms, where one
interface needs one block. With 512-frame blocks at 48 kHz that is 27 ms instead of
11 ms, with 64-frame blocks 4.6 ms instead of 1.3 ms; the delay compensation knows the
figure, so what is recorded and what plays back still line up. Use one interface (or,
on macOS, an Aggregate Device, which is one device to the host) when the monitoring
latency matters.

When a device hiccups (a USB input that delivers late or loses audio, an output that
stalls), the input is silent for the missing part, comes back at the same delay as
before, and the incident is counted; `audio.devices` reports the counts per line input
(`underruns`, `overruns`), whether the input is converted (`clock`) and the measured
difference of the clocks (`driftPpm`). A Freeze by recording during which the input had
such a gap fails and asks to be repeated. The Line inputs dialog does not show these
figures yet.

The host treats two devices as one clock only when it is certain: the same device for
input and output on macOS, the same sound card (`hw:CARD=...` and its relatives) on
Linux. Everything else, including `default`, `pulse` and `pipewire` on Linux, is
converted, which is correct for one clock too and only costs the latency above.

**Recorded files.** A recording of an input is written at the project's sample rate on
the output's clock, one frame per frame the output played, with the latency left out of
its start. From the output's own interface the frames are the device's, bit for bit; from
another device they are the converted ones (what the conversion adds is more than 100 dB
below the signal).
The file is therefore as long as the stretch of the song it was recorded against,
whatever the input's own rate and clock were.

Open the **Recording** main tab (**⌘7**) and use its embedded capture form, or use **Record sample…** above the pattern grid / **⌘⇧R**. Every ordinary sample capture defaults to a new sampler instrument plus a new tracker lane; existing instruments are preserved. Replacement requires choosing the named replacement destination explicitly. For a selected live-input
track the recorder starts with that track's interface and channels, and defaults to a
new sampler instrument and track. This preserves the original live input and its
processing. The recorder also supports replacing a sample, recording a kit Sound,
and attaching audio clips or takes. A failed import retains the recording for retry.
Offline export cannot reproduce a live hardware signal: record it first. Track Freeze records it for you
(**Record and freeze**: the song played once in real time and captured at the compensated track strip; see `docs/TRACK_FREEZE_UX.md`).
A frozen track plays its print and no longer blocks export.

For external instruments, choose **External MIDI** on the instrument page, select the
output port and MIDI channel, and enter notes normally. Playback and keyboard audition
send to the selected port even when MIDI input is disabled. A disconnected saved port
stays visible; choose its replacement explicitly. Stop/panic and route changes release
notes and pedals so a previous destination cannot keep sounding.

Automated coverage uses the null audio device with a generated input and in-memory MIDI
ports. Two clocks are tested in simulated time: an input 200 ppm fast, 200 ppm slow and
at 44.1 kHz into 48 kHz for ten minutes each without a gap or a skip, with callbacks
that come up to half a millisecond late, and stalls, bursts and lost audio on either
side. No audio input device has been opened: physical cabling, device drivers, the
microphone permission on macOS, how regular the callbacks of real devices are (which
decides how steady the conversion is and whether the margin suffices), and audible
latency still need a check on hardware.
