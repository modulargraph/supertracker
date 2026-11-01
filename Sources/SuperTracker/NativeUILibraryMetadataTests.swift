// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import AppKit
import WebKit

/// Uses the actual editor and Bridge with private generated samples. Native
/// select activation is programmatic; this does not claim physical key input.
@MainActor enum NativeUILibraryMetadataTests {
    static func run(web: WKWebView, scratch: URL, output: URL) async throws -> [String] {
        let directory = scratch.appendingPathComponent("Metadata editor samples")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let paths = ["Metadata_A_120bpm_Cmin_loop.wav", "Metadata_B_140bpm_Dmin_loop.wav"].map { directory.appendingPathComponent($0) }
        let pcm = (0..<4800).map { Float(sin(Double($0) * 0.04) * 0.1) }
        for path in paths { try AudioSamples.write(DecodedSample(path: path.path, sampleRate: 48000, left: pcm, right: pcm, originalChannels: 2), to: path) }
        let originals = try paths.map { try Data(contentsOf: $0) }
        let report = output.appendingPathComponent("library-metadata-editor-report.json")
        var evidence: [String: Any] = ["passed": false, "physicalKeyboard": false, "hardwareRecording": false]
        func write() throws { try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys]).write(to: report, options: .atomic) }
        func js(_ script: String, _ arguments: [String: Any] = [:]) async throws -> Any {
            try await web.callAsyncJavaScript(script, arguments: arguments, in: nil, contentWorld: .page) ?? NSNull()
        }
        func require(_ value: Bool, _ description: String) throws {
            if !value { throw TrackerError.message("LIBRARY EDITOR: " + description) }
        }
        func wait(_ expression: String) async throws {
            let deadline = Date().addingTimeInterval(15)
            while Date() < deadline {
                if try await js("return Boolean(\(expression));") as? Bool == true { return }
                try await Task.sleep(nanoseconds: 25_000_000)
            }
            let diagnostic = try? await js("return {modal:ST.state.modal,total:ST.state.library.total,items:ST.state.library.items,status:document.getElementById('metadata-status')?.textContent,toast:document.getElementById('toast-region')?.textContent};")
            throw TrackerError.message("LIBRARY EDITOR: Timed out: \(expression). \(String(describing: diagnostic))")
        }
        let previousFrame = web.frame
        defer { web.frame = previousFrame }
        do {
            try write()
            _ = try await js("""
            ST.closeModal(); window.__metadataProjectBefore=JSON.stringify(window.__supertracker.getProject());
            await ST.request('library.scan',{roots:[root]});
            Object.assign(ST.state.library,{query:'Metadata_',category:'',kind:'sample',filters:{},favorites:false,offset:0,selected:0});
            ST.setView('browser'); const preview=document.getElementById('browser-audition');
            preview.checked=false;preview.dispatchEvent(new Event('change',{bubbles:true}));
            await STBrowser.search();return true;
            """, ["root": directory.path])
            try await wait("!ST.state.library.scanning && ST.state.library.total===2")
            _ = try await js("await STBrowser.search();document.getElementById('browser-results-list').focus();document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',shiftKey:true,bubbles:true,cancelable:true}));document.getElementById('browser-edit-metadata').click();return true;")
            try await wait("ST.state.modal==='libraryMetadata' && document.getElementById('metadata-save')")
            try require(try await js("return document.querySelector('.modal h2')?.textContent==='Edit 2 sounds';") as? Bool == true, "Shift-arrow did not select both sounds for editing.")
            var layouts: [[String: Any]] = []
            for size in [CGSize(width: 1080, height: 700), CGSize(width: 1500, height: 940)] {
                web.frame.size = size
                // A hidden WebKit view may suspend animation frames. Poll its
                // actual viewport with the bounded native waiter instead.
                try await wait("innerWidth===\(Int(size.width)) && innerHeight===\(Int(size.height))")
                let value = try await js("""
                const ids=['metadata-save','metadata-cancel','metadata-restore'];
                const controls=ids.map(id=>{const e=document.getElementById(id),r=e.getBoundingClientRect();
                  return {id,visible:r.left>=0&&r.right<=innerWidth&&r.top>=0&&r.bottom<=innerHeight&&
                    [0.15,0.5,0.85].every(x=>[0.25,0.75].every(y=>e.contains(document.elementFromPoint(r.left+r.width*x,r.top+r.height*y))))};});
                return {width:innerWidth,height:innerHeight,controls,passed:controls.every(x=>x.visible)};
                """) as? [String: Any] ?? [:]
                try require(value["passed"] as? Bool == true, "Metadata editor actions are clipped or occluded at \(Int(size.width))×\(Int(size.height)).")
                layouts.append(value)
                evidence["layouts"] = layouts; try write()
                let configuration = WKSnapshotConfiguration(); configuration.afterScreenUpdates = true
                let image = try await web.takeSnapshot(configuration: configuration)
                if let data = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data), let png = bitmap.representation(using: .png, properties: [:]) {
                    try png.write(to: output.appendingPathComponent("library-metadata-\(Int(size.width))x\(Int(size.height)).png"))
                }
            }
            evidence["layouts"] = layouts
            _ = try await js("""
            const set=(name,value)=>{const e=document.getElementById('metadata-value-'+name);e.value=value;e.dispatchEvent(new Event(name==='sampleType'?'change':'input',{bubbles:true}));};
            set('tags','Native editor batch');set('key','F# minor');set('bpm','173');set('sampleType','oneshot');
            document.getElementById('metadata-save').click();return true;
            """)
            try await wait("!ST.state.modal && ST.state.library.items.length===2 && ST.state.library.items.every(x=>x.key==='F# minor' && x.bpm===173 && x.sampleType==='oneshot' && x.tags.includes('Native editor batch'))")
            evidence["editedItems"] = try await js("return ST.state.library.items.map(x=>({path:x.path,key:x.key,bpm:x.bpm,sampleType:x.sampleType,tags:x.tags,userMetadata:x.userMetadata}));")
            let reopened = LibraryService(cacheDirectory: scratch.appendingPathComponent("Library"), defaultRoots: [], loadCacheAsynchronously: false)
            _ = try await reopened.handle("library.scan", payload: ["roots": [directory.path]])
            var results: [String: Any] = [:]
            let deadline = Date().addingTimeInterval(10)
            repeat {
                results = try await reopened.handle("library.search", payload: ["query": "tag:\"Native editor batch\"", "filters": ["keys": ["F# minor"], "sampleTypes": ["oneshot"]]]) as? [String: Any] ?? [:]
                if results["total"] as? Int == 2 { break }
                try await Task.sleep(nanoseconds: 25_000_000)
            } while Date() < deadline
            try require(results["total"] as? Int == 2, "A fresh library did not search both persisted overrides.")
            evidence["reopenedMatchingCount"] = results["total"]
            _ = try await js("""
            document.getElementById('browser-edit-metadata').click();
            document.getElementById('metadata-restore').click();document.getElementById('metadata-save').click();return true;
            """)
            try await wait("!ST.state.modal && ST.state.library.items.length===2 && ST.state.library.items.every(x=>Object.keys(x.userMetadata).length===0 && x.sampleType==='loop')")
            try require(try await js("return JSON.stringify(window.__supertracker.getProject())===window.__metadataProjectBefore;") as? Bool == true, "Library edits changed the music project.")
            for (index, path) in paths.enumerated() { try require(try Data(contentsOf: path) == originals[index], "Metadata edits changed source sample bytes.") }
            evidence["sourceBytesUnchanged"] = true
            evidence["projectUnchanged"] = true
            evidence["restoredInferred"] = true
            evidence["passed"] = true
            try write()
            return ["Mounted batch metadata editor remains visible at both sizes and commits through the real Bridge", "Edited library metadata survives reopening, feeds native filters, restores inference and preserves source/project bytes"]
        } catch {
            evidence["error"] = error.localizedDescription
            try? write()
            throw error
        }
    }
}

#endif
