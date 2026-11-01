import Foundation

@main struct VST3CompatibilityTests {
    static func main() throws {
        func require(_ condition: Bool, _ message: String) throws {
            if !condition {
                throw NSError(domain: "VST3CompatibilityTests", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: message])
            }
        }
        let classID = "5653544E6924486D6173736976652078"
        let expected = VST3Compatibility.StartupProfile(initialFrames: 8_192,
            initialWaitNanoseconds: 1_000_000_000, beforeRestoreFrames: 512,
            componentStateWaitNanoseconds: 5_000_000_000, restoredFrames: 8_192)
        try require(VST3Compatibility.startupProfile(classID: classID, version: "1.7.1") == expected,
                    "Verified vendor version did not receive the exact bounded profile.")
        try require(VST3Compatibility.startupProfile(classID: classID.lowercased(), version: "1.7.1") == expected,
                    "Equivalent hexadecimal class-ID casing changed the profile.")
        for version in ["", "1.7", "1.7.0", "1.7.2", "1.8.0", "1.7.1.0", "v1.7.1", "1.7.1 ", " 1.7.1", "1.7.1-beta"] {
            try require(VST3Compatibility.startupProfile(classID: classID, version: version) == nil,
                        "Unverified version \(version.debugDescription) received a startup workaround.")
        }
        for other in ["", "Massive X", "vst3:" + classID, " " + classID, classID + " ",
                      String(classID.dropLast()), classID + "0", "5653544E6924486D6173736976652079",
                      "41727475415649534A756E3150726F63"] {
            try require(VST3Compatibility.startupProfile(classID: other, version: "1.7.1") == nil,
                        "A different or noncanonical class identity received the vendor profile.")
        }
        print("PASS VST3 compatibility: exact vendor/version allowlist, case-insensitive hexadecimal identity, bounded profile fields, unknown builds excluded")
    }
}
