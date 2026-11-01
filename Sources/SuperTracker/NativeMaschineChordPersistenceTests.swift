// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import WebKit

/// Extends the existing private mounted-WebKit bridge fixture. No new audio host,
/// vendor plug-in, library scan or visible window is created by this check.
@MainActor enum NativeMaschineChordPersistenceTests {
    static func run(web: WKWebView, scratch: URL, output: URL, restoreURL: URL) async throws -> [String] {
        let savedURL = scratch.appendingPathComponent("Maschine Chords.supertracker")
        guard restoreURL.standardizedFileURL.path.hasPrefix(scratch.standardizedFileURL.path + "/"),
              FileManager.default.fileExists(atPath: restoreURL.path) else {
            throw TrackerError.message("CHORD PERSISTENCE: Restore project must already exist inside private scratch storage.")
        }
        let reportURL = output.appendingPathComponent("maschine-chord-persistence-report.json")
        var evidence: [String: Any] = ["passed": false, "physicalKeyboard": false,
                                      "audibleVerification": false, "vendorPlugins": false,
                                      "privateProject": savedURL.path]
        func report() throws {
            try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys])
                .write(to: reportURL, options: .atomic)
        }
        func js(_ script: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(script, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
        }
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw TrackerError.message("CHORD PERSISTENCE: " + message) }
        }
        func wait(_ expression: String) async throws {
            let deadline = Date().addingTimeInterval(15)
            while Date() < deadline {
                if try await js("return Boolean(\(expression));") as? Bool == true { return }
                try await Task.sleep(nanoseconds: 25_000_000)
            }
            let diagnostics = try? await js("return {row:ST.state.row,recording:ST.state.recording,modal:ST.state.modal,toast:document.getElementById('toast-region')?.textContent};")
            throw TrackerError.message("CHORD PERSISTENCE: Timed out: \(expression). \(String(describing: diagnostics))")
        }
        func same(_ a: Any?, _ b: Any?) -> Bool {
            guard let a, let b else { return a == nil && b == nil }
            return (try? JSONSerialization.data(withJSONObject: a, options: [.sortedKeys, .fragmentsAllowed])) ==
                (try? JSONSerialization.data(withJSONObject: b, options: [.sortedKeys, .fragmentsAllowed]))
        }
        func restore() async throws {
            _ = try await js("""
            ST.closeModal(); ST.stopHeld(); await ST.setNoteRecording(false);
            const restored=await ST.request('project.open',{path:path});
            ST.acceptProject(restored.project||restored); await ST.syncEngine(true); return true;
            """, ["path": restoreURL.path])
        }
        try report()
        do {
            var fixture = NativeSelfTest.testProject()
            fixture["name"] = "Private Maschine chord persistence"
            fixture["masterVolume"] = 0; fixture["audioPlugins"] = []; fixture["audioClips"] = []
            fixture["patterns"] = [["id": 0, "name": "Recorded chord-set chords", "length": 16,
                "steps": [Array(repeating: NSNull(), count: 16)], "automation": []] as [String: Any]]
            fixture["sequence"] = [0]
            _ = try await js("""
            ST.closeModal(); ST.acceptProject(fixture); ST.setView('pattern');
            Object.assign(ST.state,{track:0,instrument:0,row:0,column:0,octave:4,editStep:1});
            await ST.syncEngine(true);
            const native=await ST.request('engine.snapshot',{});
            if(native.masterVolume!==0||(native.audioPlugins||[]).length)throw Error('Private chord engine must be muted and plug-in-free');
            const choose=(id,value)=>{const node=document.getElementById(id);if(!node||node.disabled)throw Error('Missing control '+id);node.focus();node.value=String(value);node.dispatchEvent(new Event('change',{bubbles:true}));};
            choose('root',0);choose('scale','wholeTone');choose('chord-set','major-inversions');
            if(ST.state.chordSet!=='major-inversions')throw Error('Chord set selection did not apply');
            document.getElementById('record').click();return true;
            """, ["fixture": fixture])
            try await wait("ST.state.recording && document.getElementById('record').getAttribute('aria-busy')!=='true'")
            _ = try await js("""
            const grid=document.getElementById('tracker');grid.focus();
            grid.dispatchEvent(new KeyboardEvent('keydown',{key:'s',code:'KeyS',bubbles:true,cancelable:true}));
            grid.dispatchEvent(new KeyboardEvent('keyup',{key:'s',code:'KeyS',bubbles:true,cancelable:true}));return true;
            """)
            try await wait("ST.pattern().steps[0][0] && ST.state.row===1")
            try require(try await js("const s=ST.pattern().steps[0][0];return s.note===47&&JSON.stringify(s.chord)==='[0,15,20,24]';") as? Bool == true,
                        "Bass walk inversions key S did not record its G/B inversion.")
            _ = try await js("""
            const set=document.getElementById('chord-set');set.value='minor-sevenths';set.dispatchEvent(new Event('change',{bubbles:true}));
            const grid=document.getElementById('tracker');grid.focus();
            grid.dispatchEvent(new KeyboardEvent('keydown',{key:'z',code:'KeyZ',bubbles:true,cancelable:true}));
            grid.dispatchEvent(new KeyboardEvent('keyup',{key:'z',code:'KeyZ',bubbles:true,cancelable:true}));return true;
            """)
            try await wait("ST.pattern().steps[0][1] && ST.state.row===2")
            try require(try await js("const s=ST.pattern().steps[0][1];return s.note===48&&JSON.stringify(s.chord)==='[0,15,19,22]';") as? Bool == true,
                        "Minor sevenths key Z did not record its Cm7 voicing.")
            _ = try await js("""
            document.getElementById('record').click();
            const root=document.getElementById('root');root.value='2';root.dispatchEvent(new Event('change',{bubbles:true}));return true;
            """)
            try await wait("!ST.state.recording")
            try require(try await js("return await ST.projectAction('save',path);", ["path": savedURL.path]) as? Bool == true,
                        "Actual Save action failed.")
            guard let saved = try await js("return window.__supertracker.getProject();") as? [String: Any] else {
                throw TrackerError.message("CHORD PERSISTENCE: Saved frontend project is missing.")
            }
            let disk = try JSONSerialization.jsonObject(with: Data(contentsOf: savedURL)) as! [String: Any]
            try require(same(disk["performance"], saved["performance"]) && same(disk["patterns"], saved["patterns"]),
                        "Disk save changed the chord-set performance or recorded chords.")
            evidence["savedPerformance"] = saved["performance"]
            evidence["savedPatterns"] = saved["patterns"]
            evidence["diskMatches"] = true
            try report()
            try require(try await js("return await ST.projectAction('open',path);", ["path": savedURL.path]) as? Bool == true,
                        "Actual private-path Open action failed.")
            try await wait("ST.state.chordSet==='minor-sevenths' && ST.state.root===2 && ST.state.pending.size===0")
            let reopened = try await js("return window.__supertracker.getProject();") as? [String: Any]
            // Opening brings the document to the current model: the UI's default built-in synths
            // become wavetable samplers (ProjectMigration). Everything else round-trips unchanged.
            try require(same(reopened, ProjectMigration.migrated(saved)), "Mounted Save/Open changed the complete project.")
            try require(try await js("return ST.state.scale==='wholeTone'&&document.getElementById('chord-set').value==='minor-sevenths'&&document.querySelectorAll('[data-chord-chip]').length===12;") as? Bool == true,
                        "Reopened chord set did not restore its controls and 12 chord labels.")
            evidence["roundTripMatches"] = true
            evidence["reopenedPerformance"] = reopened?["performance"]
            try await restore()
            evidence["restoredPriorFixture"] = true
            evidence["passed"] = true
            try report()
            return ["Actual factory chord selection and keyboard inversions survive private Save, disk validation and Open"]
        } catch {
            evidence["error"] = error.localizedDescription
            do { try await restore(); evidence["restoredPriorFixture"] = true }
            catch { evidence["restoreError"] = error.localizedDescription }
            try? report()
            throw error
        }
    }
}

#endif
