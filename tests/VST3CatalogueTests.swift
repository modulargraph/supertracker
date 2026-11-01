import Foundation
import Darwin

private struct Failure: Error { let message: String }

/// A deterministic disposable helper exercises process isolation, deadlines and
/// cache policy without requiring any particular vendor's plug-in installation.
@main struct VST3CatalogueTests {
    static let firstID = "00112233445566778899AABBCCDDEEFF"
    static let secondID = "FFEEDDCCBBAA99887766554433221100"

    static func require(_ value: @autoclosure () -> Bool, _ message: String) throws {
        if !value() { throw Failure(message: message) }
    }
    static func info(_ bundle: URL) throws -> [String: Any] {
        try PropertyListSerialization.propertyList(from: Data(contentsOf: bundle.appendingPathComponent("Contents/Info.plist")), options: [], format: nil) as! [String: Any]
    }
    static func updateInfo(_ bundle: URL, _ change: (inout [String: Any]) -> Void) throws {
        var value = try info(bundle); change(&value)
        try PropertyListSerialization.data(fromPropertyList: value, format: .xml, options: 0)
            .write(to: bundle.appendingPathComponent("Contents/Info.plist"), options: .atomic)
    }
    static func makeBundle(_ root: URL, name: String, id: String, mode: String = "ok", instrument: Bool = true) throws -> URL {
        let bundle = root.appendingPathComponent(name + ".vst3")
        try FileManager.default.createDirectory(at: bundle.appendingPathComponent("Contents/MacOS"), withIntermediateDirectories: true)
        let value: [String: Any] = ["CFBundleExecutable": "test-plugin", "CFBundleVersion": "1", "CFBundleShortVersionString": "1.0",
                                    "TestClassID": id, "TestMode": mode, "TestInstrument": instrument]
        try PropertyListSerialization.data(fromPropertyList: value, format: .xml, options: 0).write(to: bundle.appendingPathComponent("Contents/Info.plist"))
        try Data([1, 2, 3]).write(to: bundle.appendingPathComponent("Contents/MacOS/test-plugin"))
        return bundle
    }
    static func count(_ bundle: URL) -> Int {
        Int((try? String(contentsOf: bundle.appendingPathComponent("scan-count"), encoding: .utf8)) ?? "0") ?? 0
    }
    static func helper(_ bundle: URL, output: URL) throws {
        let metadata = try info(bundle), mode = metadata["TestMode"] as! String
        try String(count(bundle) + 1).write(to: bundle.appendingPathComponent("scan-count"), atomically: true, encoding: .utf8)
        try String(getpid()).write(to: bundle.appendingPathComponent("scan-pid"), atomically: true, encoding: .utf8)
        // A noisy or wedged factory must not deadlock the parent or fill its memory.
        if mode == "noisy" { FileHandle.standardOutput.write(Data(repeating: 65, count: 4 * 1_024 * 1_024)) }
        if mode == "failed" { exit(23) }
        if mode == "hang" { signal(SIGTERM, SIG_IGN); while true { usleep(10_000) } }
        if mode == "hold" {
            while !FileManager.default.fileExists(atPath: bundle.appendingPathComponent("release").path) { usleep(10_000) }
        }
        if mode == "oversize" { try Data(repeating: 1, count: 3 * 1_024 * 1_024).write(to: output); return }
        let instrument = metadata["TestInstrument"] as! Bool
        let descriptor = VST3PluginDescriptor(classID: mode == "invalid" ? "NOT-A-CLASS-ID" : metadata["TestClassID"] as! String,
            bundlePath: bundle.path, name: bundle.deletingPathExtension().lastPathComponent, manufacturer: "Fixture",
            version: metadata["CFBundleShortVersionString"] as! String, category: instrument ? "Instrument|Synth" : "Fx|Delay")
        try JSONEncoder().encode(VST3Catalogue.ScanEnvelope(schema: 1, bundlePath: bundle.path, records: [descriptor], error: nil))
            .write(to: output, options: .atomic)
    }
    static func waitFor(_ condition: () -> Bool) async throws {
        for _ in 0..<500 {
            if condition() { return }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        throw Failure(message: "Timed out waiting for helper checkpoint")
    }

    static func main() async throws {
        if let index = CommandLine.arguments.firstIndex(of: "--vst3-scan"),
           let outputIndex = CommandLine.arguments.firstIndex(of: "--output") {
            try helper(URL(fileURLWithPath: CommandLine.arguments[index + 1]), output: URL(fileURLWithPath: CommandLine.arguments[outputIndex + 1])); return
        }
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-vst3-tests-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: folder) }
        let user = folder.appendingPathComponent("User"), system = folder.appendingPathComponent("System")
        let userSynth = try makeBundle(user, name: "A Synth", id: firstID, mode: "noisy")
        let systemSynth = try makeBundle(system, name: "A Synth System", id: firstID)
        let effect = try makeBundle(system, name: "Delay", id: secondID, instrument: false)
        let failed = try makeBundle(system, name: "Failed", id: secondID, mode: "failed")
        let hanging = try makeBundle(system, name: "Hang", id: secondID, mode: "hang")
        let invalid = try makeBundle(system, name: "Invalid", id: secondID, mode: "invalid")
        let oversize = try makeBundle(system, name: "Oversize", id: secondID, mode: "oversize")
        var configuration = VST3Catalogue.Configuration()
        configuration.roots = [user, system]; configuration.cacheURL = folder.appendingPathComponent("Cache/catalogue.json")
        configuration.timeout = 0.2
        let catalogue = VST3Catalogue(configuration: configuration)
        let started = ProcessInfo.processInfo.systemUptime
        let report = await catalogue.refresh()
        try require(ProcessInfo.processInfo.systemUptime - started < 8, "A hung factory blocked discovery")
        try require(report.scanned == 7 && report.errors.count == 4, "Failed helpers or scan accounting were lost")
        try require(report.records.count == 2, "Catalogue did not deduplicate actual class IDs")
        try require(catalogue.resolve(identifier: "vst3:" + firstID.lowercased())?.bundlePath == userSynth.path, "User installation did not take deterministic priority")
        try require(catalogue.resolve(identifier: "VST3:" + firstID.lowercased())?.bundlePath == userSynth.path, "Format prefix casing changed class identity")
        try require(catalogue.resolve(identifier: firstID, bundlePath: systemSynth.path)?.bundlePath == systemSynth.path, "Explicit compatible bundle path was ignored")
        try require(catalogue.resolve(identifier: secondID, bundlePath: systemSynth.path) == nil, "Explicit path mismatch fell back to a different installation")
        try require(catalogue.records.first { $0.classID == secondID }?.isInstrument == false, "Effect classification was lost")
        try require(catalogue.records.first { $0.classID == firstID }?.dictionary["format"] as? String == "VST3", "Format discriminator missing")
        let deadPID = Int32(try String(contentsOf: hanging.appendingPathComponent("scan-pid"), encoding: .utf8))!
        try require(Darwin.kill(deadPID, 0) == -1 && errno == ESRCH, "Timeout left the hung child alive")

        let snapshot = catalogue.records
        let reopened = VST3Catalogue(configuration: configuration)
        try require(reopened.records == snapshot, "Cached classes were not immediately available")
        let immediate = try await reopened.resolveOrScan(identifier: secondID)
        try require(immediate.bundlePath == effect.path && count(effect) == 1, "Cached NKS resolution reopened a factory")
        let cached = await reopened.refresh()
        try require(cached.cached == 7 && cached.scanned == 0 && cached.errors.count == 4, "Cache did not retain successes and quarantine unchanged failures")
        try require([userSynth, systemSynth, effect, failed, hanging, invalid, oversize].allSatisfy { count($0) == 1 }, "Cached refresh opened a factory")

        try Data([1, 2, 3, 4]).write(to: effect.appendingPathComponent("Contents/MacOS/test-plugin"))
        let binaryChanged = await reopened.refresh()
        try require(binaryChanged.scanned == 1 && count(effect) == 2, "Executable fingerprint did not invalidate its cache")
        try updateInfo(userSynth) { $0["CFBundleShortVersionString"] = "2.0" }
        let versionChanged = await reopened.refresh()
        try require(versionChanged.scanned == 1 && reopened.resolve(identifier: firstID)?.version == "2.0", "Version change did not invalidate metadata")

        // Refresh requests coalesce while an isolated helper is suspended.
        let isolatedRoot = folder.appendingPathComponent("Isolated")
        let held = try makeBundle(isolatedRoot, name: "Held", id: firstID, mode: "hold")
        var heldConfiguration = configuration
        heldConfiguration.roots = [isolatedRoot]; heldConfiguration.timeout = 5
        heldConfiguration.cacheURL = folder.appendingPathComponent("HeldCache.json")
        let shared = VST3Catalogue(configuration: heldConfiguration)
        let first = Task { await shared.refresh() }
        try await waitFor { count(held) == 1 }
        let second = Task { await shared.refresh() }
        try await Task.sleep(nanoseconds: 50_000_000)
        try Data().write(to: held.appendingPathComponent("release"))
        let firstResult = await first.value, secondResult = await second.value
        try require(count(held) == 1 && firstResult.records == secondResult.records, "Concurrent refreshes did not share one child")
        try require(firstResult.records.count == 1, "Coalesced result was empty")

        try FileManager.default.removeItem(at: held.appendingPathComponent("release"))
        let cancelledScan = Task { await shared.refresh(force: true) }
        try await waitFor { count(held) == 2 }
        shared.cancel()
        let cancelled = await cancelledScan.value
        try require(cancelled.cancelled && cancelled.records == firstResult.records, "Cancellation discarded the last known catalogue")
        let cancelledPID = Int32(try String(contentsOf: held.appendingPathComponent("scan-pid"), encoding: .utf8))!
        try require(Darwin.kill(cancelledPID, 0) == -1 && errno == ESRCH, "Cancellation left its child alive")

        // An NKS name is a scheduling hint, never substitute identity. Return a
        // verified requested class before unrelated, slow factories finish.
        let priorityRoot = folder.appendingPathComponent("Priority")
        let slow = try makeBundle(priorityRoot, name: "A Slow", id: firstID, mode: "hold")
        let desired = try makeBundle(priorityRoot, name: "Z Desired", id: secondID)
        var priorityConfiguration = heldConfiguration
        priorityConfiguration.roots = [priorityRoot]
        priorityConfiguration.cacheURL = folder.appendingPathComponent("PriorityCache.json")
        let priority = VST3Catalogue(configuration: priorityConfiguration)
        let lookupStarted = ProcessInfo.processInfo.systemUptime
        let resolved = try await priority.resolveOrScan(identifier: secondID, preferredNames: ["Z Desired"])
        try require(resolved.classID == secondID && ProcessInfo.processInfo.systemUptime - lookupStarted < 2, "NKS hint did not prioritize its factory")
        try require(count(desired) == 1, "Prioritized factory was scanned more than once")
        priority.cancel()
        try await waitFor { priority.status["scanning"] as? Bool == false }
        if let text = try? String(contentsOf: slow.appendingPathComponent("scan-pid"), encoding: .utf8), let pid = Int32(text) {
            try require(Darwin.kill(pid, 0) == -1 && errno == ESRCH, "Priority lookup left its background child alive")
        }
        try Data().write(to: slow.appendingPathComponent("release"))
        let actual = try await priority.resolveOrScan(identifier: firstID, preferredNames: ["Z Desired"])
        try require(actual.classID == firstID && actual.bundlePath == slow.path, "NKS display name was incorrectly treated as class identity")

        try FileManager.default.removeItem(at: userSynth)
        _ = await reopened.refresh()
        try require(reopened.resolve(identifier: firstID)?.bundlePath == systemSynth.path, "Removed user plug-in hid the valid system installation")
        try require(snapshot.first { $0.classID == firstID }?.bundlePath == userSynth.path, "Previously returned snapshot was mutated")

        var targetedConfiguration = configuration
        targetedConfiguration.cacheURL = folder.appendingPathComponent("TargetedCache.json")
        let targeted = VST3Catalogue(configuration: targetedConfiguration)
        let effectCount = count(effect), failedCount = count(failed)
        let single = try await targeted.scanBundle(effect)
        try require(single.count == 1 && targeted.records == single && count(effect) == effectCount + 1,
                    "Targeted scan did not merge exactly one factory")
        try require(count(failed) == failedCount, "Targeted scan opened an unrelated module")
        _ = try await targeted.scanBundle(effect)
        try require(count(effect) == effectCount + 1, "Targeted scan ignored the executable cache")
        try require(VST3Catalogue(configuration: targetedConfiguration).resolve(identifier: secondID) == single[0],
                    "Targeted scan did not persist verified class identity")
        let targetedHeld = Task { try await targeted.scanBundle(held, force: true) }
        try await waitFor { count(held) == 3 }
        targetedHeld.cancel()
        do { _ = try await targetedHeld.value; throw Failure(message: "Cancelled targeted scan returned a result") }
        catch is CancellationError { }
        let targetedPID = Int32(try String(contentsOf: held.appendingPathComponent("scan-pid"), encoding: .utf8))!
        try require(Darwin.kill(targetedPID, 0) == -1 && errno == ESRCH, "Task cancellation left targeted child alive")
        // Saved rack/recovery descriptors bind both class ID and installed location.
        // A duplicate class at another path is not an acceptable silent substitute.
        let exactRoot = folder.appendingPathComponent("ExactIdentity")
        let exactUserRoot = exactRoot.appendingPathComponent("User"), exactSystemRoot = exactRoot.appendingPathComponent("System")
        let exactUser = try makeBundle(exactUserRoot, name: "Same Synth", id: firstID)
        let exactSystem = try makeBundle(exactSystemRoot, name: "Same Synth", id: firstID)
        let wrongClass = try makeBundle(exactSystemRoot, name: "Other Synth", id: secondID)
        try updateInfo(exactSystem) { $0["CFBundleShortVersionString"] = "2.1" }
        var exactConfiguration = configuration
        exactConfiguration.roots = [exactUserRoot, exactSystemRoot]
        exactConfiguration.cacheURL = exactRoot.appendingPathComponent("Cache.json")
        let exact = VST3Catalogue(configuration: exactConfiguration)
        let selected = try await exact.resolveOrScan(identifier: "VST3:" + firstID.lowercased(), preferredNames: ["Same Synth"],
                                                     preferredBundlePath: exactSystem.path)
        try require(selected.bundlePath == exactSystem.path && selected.version == "2.1", "Explicit uncached location selected the wrong duplicate/version")
        try require(count(exactSystem) == 1 && count(exactUser) == 0 && count(wrongClass) == 0,
                    "Explicit location triggered discovery of other installations")
        _ = try await exact.scanBundle(exactUser)
        let preferred = try await exact.resolveOrScan(identifier: firstID)
        try require(preferred.bundlePath == exactUser.path, "Nil explicit location no longer uses user-first selection")
        let cachedExplicit = try await exact.resolveOrScan(identifier: firstID, preferredBundlePath: exactSystem.path)
        try require(cachedExplicit == selected && count(exactSystem) == 1, "Explicit cached duplicate was replaced or unnecessarily rescanned")
        func rejectsExact(_ path: String) async throws {
            do {
                _ = try await exact.resolveOrScan(identifier: firstID, preferredBundlePath: path)
            } catch { return }
            throw Failure(message: "Explicit missing/mismatched location fell back to another installation: \(path)")
        }
        try await rejectsExact(wrongClass.path)
        try require(count(wrongClass) == 1 && count(exactSystem) == 1 && count(exactUser) == 1,
                    "Class mismatch scanned more than the requested factory")
        try await rejectsExact(wrongClass.path)
        try require(count(wrongClass) == 1, "Cached class mismatch reopened its factory")
        try await rejectsExact(exactRoot.appendingPathComponent("Missing.vst3").path)
        try await rejectsExact("relative/Same Synth.vst3")
        try updateInfo(exactSystem) { $0["CFBundleShortVersionString"] = "2.2" }
        let updatedExplicit = try await exact.resolveOrScan(identifier: firstID, preferredBundlePath: exactSystem.path)
        try require(updatedExplicit.version == "2.2" && count(exactSystem) == 2, "Explicit cache retained a replaced installation's old version")
        try FileManager.default.removeItem(at: exactSystem)
        try await rejectsExact(exactSystem.path)
        try require(exact.resolve(identifier: firstID)?.bundlePath == exactUser.path, "Rejected explicit location altered the unrelated user installation")
        // Several releases can replace one VST2: the newest class wins regardless of scan order.
        let magic: UInt32 = 0x4E694D61
        func compatible(_ suffix: String, _ version: String, _ path: String, instrument: Bool = true) -> VST3PluginDescriptor {
            VST3PluginDescriptor(classID: "565354" + "4E694D61" + suffix, bundlePath: path, name: "Compat " + version, manufacturer: "Fixture",
                version: version, category: instrument ? "Instrument|Synth" : "Fx")
        }
        let seven = compatible("6D6173736976650001", "7.10.0", "/Library/Audio/Plug-Ins/VST3/Seven.vst3")
        let eight = compatible("6D6173736976650002", "8.0.1", "/Library/Audio/Plug-Ins/VST3/Eight.vst3")
        let oldSeven = compatible("6D6173736976650001", "9.9", "/Library/Audio/Plug-Ins/VST3/OldSeven.vst3")
        let compatibleEffect = compatible("6D6173736976650003", "10.0", "/Library/Audio/Plug-Ins/VST3/Effect.vst3", instrument: false)
        for order in [[seven, eight, oldSeven, compatibleEffect], [compatibleEffect, oldSeven, eight, seven].reversed(), [eight, seven]] {
            try require(VST3Catalogue.preferredCompatible(Array(order), vst2Magic: magic, isInstrument: true) == eight,
                        "Compatible VST3 resolution depended on scan order instead of the newest release")
        }
        try require(VST3Catalogue.preferredCompatible([seven, compatibleEffect], vst2Magic: magic, isInstrument: false) == compatibleEffect &&
                    VST3Catalogue.preferredCompatible([seven], vst2Magic: magic + 1) == nil, "Compatible VST3 filters changed")
        let twin = compatible("6D6173736976650000", "7.10.0", "/Z.vst3")
        try require(VST3Catalogue.preferredCompatible([seven, twin], vst2Magic: magic) == twin &&
                    VST3Catalogue.preferredCompatible([twin, seven], vst2Magic: magic) == twin, "Equal versions are not ordered deterministically")
        try require(VST3Catalogue.compareVersions("1.10", "1.9") == .orderedDescending && VST3Catalogue.compareVersions("2", "2.0") == .orderedAscending &&
                    VST3Catalogue.compareVersions("3.1.0", "3.1.0") == .orderedSame, "Version ordering is not numeric")
        // NKS identity policy: exact VST3 class, else the same preset's native VST2.
        struct Missing: Error {}
        func nks(_ classID: String?, _ magic: UInt32?, vst3: Bool, vst2: Bool) async throws -> String {
            try await VST3Catalogue.resolveNKSComponent(vst3ClassID: classID, vst2Magic: magic,
                exactVST3: { id in if vst3 { return id }; throw Missing() },
                nativeVST2: { value in vst2 ? String(format: "vst2:%08X", value) : nil },
                compatibleVST3: { _ in "vst3:COMPATIBLE" })
        }
        let exactChoice = try await nks(firstID, magic, vst3: true, vst2: true), fallback = try await nks(firstID, magic, vst3: false, vst2: true)
        let compatibleChoice = try await nks(nil, magic, vst3: false, vst2: false), nativeChoice = try await nks(nil, magic, vst3: false, vst2: true)
        try require(exactChoice == "vst3:" + firstID, "Installed exact VST3 was not preferred")
        try require(fallback == "vst2:4E694D61", "Missing VST3 did not fall back to the installed VST2")
        try require(compatibleChoice == "vst3:COMPATIBLE" && nativeChoice == "vst2:4E694D61", "VST2-only presets changed resolution order")
        for (classID, value): (String?, UInt32?) in [(firstID, nil), (firstID, magic)] {
            do { _ = try await nks(classID, value, vst3: false, vst2: false); throw Failure(message: "A missing exact VST3 loaded something else") }
            catch is Missing {}
        }
        // Ordering a large catalogue is linearithmic: root priority is not recomputed per comparison.
        let roots = (0..<40).map { URL(fileURLWithPath: "/Library/Audio/Plug-Ins/Root\($0)") }
        let many = (0..<20_000).map { index in
            VST3PluginDescriptor(classID: String(format: "%032X", index % 15_000), bundlePath: roots[index % 40].path + "/P\(index).vst3",
                name: "Plug-in \(19_999 - index)", manufacturer: "Fixture", version: "1", category: "Fx", isInstrument: false)
        }
        let orderingStarted = Date()
        let ordered = VST3Catalogue.preferred(many, roots: roots)
        let elapsed = Date().timeIntervalSince(orderingStarted)
        guard ordered.count == 15_000, Set(ordered.map(\.id)).count == 15_000, elapsed < 5,
              zip(ordered, ordered.dropFirst()).allSatisfy({ $0.name.localizedStandardCompare($1.name) != .orderedDescending }) else {
            throw NSError(domain: "VST3CatalogueTests", code: 9, userInfo: [NSLocalizedDescriptionKey: "Large catalogue ordering is wrong or slow (\(elapsed) s)."])
        }
        // Duplicate class IDs keep the copy in the earliest root.
        let earlier = ordered.first { $0.id == "vst3:" + String(format: "%032X", 3) }
        guard earlier?.bundlePath.hasPrefix(roots[3].path + "/") == true else {
            throw NSError(domain: "VST3CatalogueTests", code: 10, userInfo: [NSLocalizedDescriptionKey: "Root priority lost while de-duplicating."])
        }
        print("PASS 20000-class catalogue ordered in \(String(format: "%.2f", elapsed)) s")
        print("PASS VST3 catalogue: actual-class identity, user/system priority, immutable cached snapshots, executable/version invalidation, isolated bounded discovery, cancellation/coalescing, NKS resolution, and strict explicit-location class/version identity without substitution")
    }
}
