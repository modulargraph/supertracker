//! Component listing for the plug-in browser (macOS only): reads the system's Audio Unit
//! registry through `AVAudioUnitComponentManager`. No plug-in code is loaded.

use objc2_audio_toolbox::{AudioComponentDescription, AudioComponentFlags};
use objc2_avf_audio::{AVAudioUnitComponent, AVAudioUnitComponentManager};

use super::ident::{self, ComponentId, ComponentInfo, ComponentKind, FourCc};
use super::objc::guard_quiet;

/// The types the browser lists. Generators can be hosted but are not offered.
const LISTED: [ComponentKind; 4] =
    [ComponentKind::Instrument, ComponentKind::Effect, ComponentKind::MusicEffect, ComponentKind::MidiEffect];

pub(super) fn list_components() -> Vec<ComponentInfo> {
    let mut list = Vec::new();
    for kind in LISTED {
        // Zero subtype, manufacturer and flags mask: every component of this type.
        let query = AudioComponentDescription {
            componentType: kind.type_code().0,
            componentSubType: 0,
            componentManufacturer: 0,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        let found = guard_quiet("listing the installed Audio Units", || {
            // SAFETY: plain queries of the shared component manager.
            let components = unsafe {
                AVAudioUnitComponentManager::sharedAudioUnitComponentManager().componentsMatchingDescription(query)
            };
            components.iter().filter_map(|component| describe(&component, kind)).collect::<Vec<_>>()
        });
        list.extend(found.unwrap_or_default());
    }
    ident::finish_listing(list)
}

fn describe(component: &AVAudioUnitComponent, kind: ComponentKind) -> Option<ComponentInfo> {
    // SAFETY: property reads of an immutable description object.
    unsafe {
        let description = component.audioComponentDescription();
        if description.componentType != kind.type_code().0 {
            return None;
        }
        let id = ComponentId {
            kind: FourCc(description.componentType),
            subtype: FourCc(description.componentSubType),
            manufacturer: FourCc(description.componentManufacturer),
        };
        let flags = AudioComponentFlags(description.componentFlags);
        let version = component.version() as u32;
        let version_string = component.versionString().to_string();
        Some(ComponentInfo {
            plugin_id: id.to_string(),
            name: component.name().to_string(),
            manufacturer: component.manufacturerName().to_string(),
            kind,
            type_name: component.typeName().to_string(),
            version,
            version_string: if version_string.is_empty() { ident::version_string(version) } else { version_string },
            has_custom_view: component.hasCustomView(),
            tags: component.allTagNames().iter().map(|tag| tag.to_string()).collect(),
            has_midi_input: component.hasMIDIInput(),
            has_midi_output: component.hasMIDIOutput(),
            is_v3: flags.contains(AudioComponentFlags::IsV3AudioUnit),
            sandbox_safe: component.isSandboxSafe(),
        })
    }
}
