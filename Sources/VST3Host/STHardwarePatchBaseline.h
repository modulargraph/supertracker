#pragma once
#include "pluginterfaces/base/funknown.h"

// Optional Studio Tools extension. Query IAudioProcessor with this IID. Calls
// run on the host control thread. Capture requires quiescent audio; capability
// reads and individual base-field edits must tolerate concurrent processing. Existing VST3
// parameter IDs/order and standard opaque state containers remain unchanged.
namespace StudioTools {
inline constexpr Steinberg::TUID hardwarePatchBaselineIID = {
    0x53,0x54,0x42,0x50,0x31,0x46,0x47,0x61,0x22,0x16,0x39,0x11,0x64,0x03,0x25,0x01
};
enum HardwarePatchBaselineCapability : Steinberg::uint32 {
    stateIsBaseline = 1u << 0,
    captureReady = 1u << 1,
    manualParameterEdits = 1u << 2
};
class IHardwarePatchBaseline : public Steinberg::FUnknown {
public:
    virtual Steinberg::uint32 PLUGIN_API protocolVersion() = 0;
    virtual Steinberg::uint32 PLUGIN_API baselineCapabilities() = 0;
    // Promote the current cached live patch; never read/reset/send hardware.
    virtual Steinberg::tresult PLUGIN_API captureBaseline() = 0;
    // Host UI edits update just this base field; playback automation does not.
    // id is the existing VST3 ParamID, value is normalized. No MIDI is sent here;
    // the ordinary parameter queue delivers the corresponding live edit.
    virtual Steinberg::tresult PLUGIN_API setBaselineParameter(Steinberg::uint32 id, double value) = 0;
};
}
