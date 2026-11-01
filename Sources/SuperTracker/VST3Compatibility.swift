/// Bounded startup workarounds established for particular installed vendor builds.
/// These empirical timings are not a universal VST3 initialization policy and must
/// not silently apply to other instruments or unverified versions of the same one.
enum VST3Compatibility {
    struct StartupProfile: Equatable {
        let initialFrames: UInt32
        let initialWaitNanoseconds: UInt64
        let beforeRestoreFrames: UInt32
        let componentStateWaitNanoseconds: UInt64
        let restoredFrames: UInt32
    }

    static func startupProfile(classID: String, version: String) -> StartupProfile? {
        guard classID.uppercased() == "5653544E6924486D6173736976652078", version == "1.7.1" else { return nil }
        return StartupProfile(initialFrames: 8_192, initialWaitNanoseconds: 1_000_000_000,
                              beforeRestoreFrames: 512, componentStateWaitNanoseconds: 5_000_000_000,
                              restoredFrames: 8_192)
    }
}
