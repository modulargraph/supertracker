//! The state of a VST3 instance as one byte blob.
//!
//! VST3 keeps two states: the component's (the processor's parameters, what a preset is)
//! and the edit controller's (things only the editor knows). The host stores both, and on
//! restore gives the component state to the component *and* to the controller
//! (`setComponentState`) before the controller's own state.
//!
//! ```text
//! offset  size  field
//!      0     4  magic "STV3"
//!      4     2  format version, little-endian (currently 1)
//!      6     2  flags, little-endian: bit 0 = a controller state follows the component state
//!      8    16  class id of the component: the 16 bytes that read as the 32 hex digits
//!     24     8  length of the component state, little-endian
//!     32     8  length of the controller state, little-endian (0 without bit 0)
//!     40     …  component state, then controller state
//! ```
//!
//! A blob for another class, of a newer format version, or whose lengths do not add up to
//! its size is refused before the plug-in sees a byte of it.

pub const MAGIC: &[u8; 4] = b"STV3";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 40;
const FLAG_CONTROLLER: u16 = 1;

/// The parts of a state blob.
#[derive(Debug, PartialEq)]
pub struct Parts<'a> {
    pub component: &'a [u8],
    /// `None`: the controller had no state of its own when the blob was made.
    pub controller: Option<&'a [u8]>,
}

/// `class_id`: the class id as the four words hosts print (see `module::cid_words`).
pub fn encode(class_id: [u32; 4], component: &[u8], controller: Option<&[u8]>) -> Vec<u8> {
    let controller_len = controller.map_or(0, <[u8]>::len);
    let mut out = Vec::with_capacity(HEADER_LEN + component.len() + controller_len);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(if controller.is_some() { FLAG_CONTROLLER } else { 0 }).to_le_bytes());
    for word in class_id {
        out.extend_from_slice(&word.to_be_bytes());
    }
    out.extend_from_slice(&(component.len() as u64).to_le_bytes());
    out.extend_from_slice(&(controller_len as u64).to_le_bytes());
    out.extend_from_slice(component);
    out.extend_from_slice(controller.unwrap_or(&[]));
    out
}

pub fn decode(class_id: [u32; 4], blob: &[u8]) -> Result<Parts<'_>, String> {
    if blob.len() < HEADER_LEN || &blob[..4] != MAGIC {
        return Err("this is not a state saved by the VST3 host".into());
    }
    let version = u16::from_le_bytes([blob[4], blob[5]]);
    if version == 0 || version > VERSION {
        return Err(format!("the state has format version {version}; this host reads up to {VERSION}"));
    }
    let flags = u16::from_le_bytes([blob[6], blob[7]]);
    let mut expected = [0u8; 16];
    for (index, word) in class_id.iter().enumerate() {
        expected[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    if blob[8..24] != expected {
        let found: String = blob[8..24].iter().map(|byte| format!("{byte:02X}")).collect();
        return Err(format!("the state belongs to another plug-in (VST3 class {found})"));
    }
    let length = |offset: usize| u64::from_le_bytes(blob[offset..offset + 8].try_into().unwrap());
    let (component, controller) = (length(24), length(32));
    let body = (blob.len() - HEADER_LEN) as u64;
    if component > body || controller != body - component || (flags & FLAG_CONTROLLER == 0 && controller != 0) {
        return Err("the state is truncated or corrupt".into());
    }
    let split = HEADER_LEN + component as usize;
    Ok(Parts {
        component: &blob[HEADER_LEN..split],
        controller: (flags & FLAG_CONTROLLER != 0).then(|| &blob[split..]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASS: [u32; 4] = [0x5354_5633, 0x4761_696E, 0x5072_6F63, 1];

    #[test]
    fn round_trips_with_and_without_controller_state() {
        let blob = encode(CLASS, b"component", Some(b"ctrl"));
        assert_eq!(blob.len(), HEADER_LEN + 13);
        assert_eq!(&blob[..8], b"STV3\x01\x00\x01\x00");
        assert_eq!(&blob[8..12], b"STV3", "class id bytes read as the hex digits");
        assert_eq!(decode(CLASS, &blob).unwrap(), Parts { component: b"component", controller: Some(b"ctrl") });
        let blob = encode(CLASS, b"component", None);
        assert_eq!(decode(CLASS, &blob).unwrap(), Parts { component: b"component", controller: None });
        // An empty controller state is not the same as none.
        let blob = encode(CLASS, b"", Some(b""));
        assert_eq!(decode(CLASS, &blob).unwrap(), Parts { component: b"", controller: Some(b"") });
    }

    #[test]
    fn refuses_foreign_newer_and_damaged_blobs() {
        let blob = encode(CLASS, b"component", Some(b"ctrl"));
        assert!(decode([1, 2, 3, 4], &blob).unwrap_err().contains("another plug-in (VST3 class 535456334761696E50726F6300000001)"));
        for cut in 0..blob.len() {
            assert!(decode(CLASS, &blob[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = blob.clone();
        longer.push(0);
        assert!(decode(CLASS, &longer).is_err());
        let mut newer = blob.clone();
        newer[4] = 2;
        assert!(decode(CLASS, &newer).unwrap_err().contains("format version 2"));
        let mut lying = blob.clone();
        lying[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(decode(CLASS, &lying).is_err());
        let mut unflagged = blob.clone();
        unflagged[6] = 0;
        assert!(decode(CLASS, &unflagged).is_err());
        assert!(decode(CLASS, b"").is_err());
        assert!(decode(CLASS, b"not a state").is_err());
    }
}
