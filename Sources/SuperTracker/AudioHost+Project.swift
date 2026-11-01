import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Publishing projects to the engine, the plug-in rack and the routing graph; unavailable-routing sessions.
extension AudioHost {
    /// Main thread: give the graph strips, native sources and meter taps for a project
    /// with this many tracks. Growth attaches new nodes only; the running graph is
    /// never stopped or paused, and existing strips are never torn down.
    @MainActor func ensureChannelStrips(_ data:[String:Any]) throws {
        let tracks = max(1,(data["tracks"] as? [[String:Any]])?.count ?? 1)
        guard tracks <= Int(ST_TRACK_SANITY_LIMIT) else { throw AudioSampleError.invalid("A project supports at most \(ST_TRACK_SANITY_LIMIT) tracks.") }
        try rack.ensureStrips(channels:Int(st_channel_count(Int32(tracks))),tracks:tracks) { channel in
            let node = self.makeNativeSource(channel:Int32(channel)); self.sources.insert(node,at:max(0,self.sources.count-1)); return node
        }
        let metered = min(Int(st_return_bus(Int32(tracks))),rack.realizedStrips)
        while meterTaps < metered {
            let track = meterTaps
            rack.outputNode(track:track)?.installTap(onBus:0,bufferSize:512,format:nil) { [weak self] buffer,time in
                self?.meters.measure(buffer,bus:track)
                self?.freezeTrackRuntime.tap.receive(buffer,at:time,track:track)
            }
            meterTaps += 1
        }
    }

    @MainActor func applyEngineProject(_ data:[String:Any]) throws {
        try AudioProject.apply(AudioFreezeTrack.engineProject(data),to:engine,clipAssets:clipAssetSlots,sampleRate:sampleRate)
    }

    /// Everything a publication's routing graph, plug-in reconciliation and lock
    /// validation read: the project without its pattern step grids, plus every
    /// plug-in step-FX lock with its position. Nil when a grid is malformed.
    nonisolated static func publicationShape(_ data:[String:Any]) -> (structure:NSDictionary, locks:NSArray)? {
        var copy = data, locks:[Any] = [], sendUsage = Set<Int>()
        if let value = data["patterns"] {
            guard let patterns = value as? [[String:Any]] else { return nil }
            var stripped:[[String:Any]] = []
            stripped.reserveCapacity(patterns.count)
            for (index,pattern) in patterns.enumerated() {
                var item = pattern
                if let value = item.removeValue(forKey:"steps") {
                    guard let grid = value as? [[Any]] else { return nil }
                    for (track,rows) in grid.enumerated() {
                        for (row,raw) in rows.enumerated() {
                            guard let step = raw as? [String:Any] else { continue }
                            for key in ["fx1","fx2"] {
                                guard let fx = step[key] as? [String:Any] else { continue }
                                switch fx["type"] as? String {
                                case "plugin": locks.append([index,track,row,key,fx] as [Any])
                                // Which tracks use delay/reverb step FX decides their send routes
                                // (AudioPluginRack.builtinSenders): a change needs a full publication.
                                case "delay": sendUsage.insert(track * 2)
                                case "reverb": sendUsage.insert(track * 2 + 1)
                                default: break
                                }
                            }
                        }
                    }
                    item["stepTracks"] = grid.count
                }
                stripped.append(item)
            }
            copy["patterns"] = stripped
        }
        // A track's note-column count only changes how its steps publish, like the steps themselves.
        if let tracks = data["tracks"] as? [[String:Any]], tracks.contains(where: { $0["noteColumns"] != nil }) {
            copy["tracks"] = tracks.map { track -> [String:Any] in var item = track; item.removeValue(forKey:"noteColumns"); return item }
        }
        if !sendUsage.isEmpty { locks.append(["sendFX", sendUsage.sorted()] as [Any]) }
        return (copy as NSDictionary, locks as NSArray)
    }

    /// A note edit while playing publishes the whole project. When only pattern
    /// step data changed (same routing, plug-ins, instruments, automation and
    /// plug-in locks as the project already applied), the routing plan, plug-in
    /// reconciliation and lock validation would all reproduce the current state:
    /// publish the sequencer data alone. Never touches the running graph.
    @MainActor func applyPatternDataOnly(_ data:[String:Any]) throws -> Bool {
        guard routingFailure == nil, unavailableProject == nil, !project.isEmpty,
              let next = Self.publicationShape(data), let current = Self.publicationShape(project),
              next.structure.isEqual(current.structure), next.locks.isEqual(current.locks),
              mediaSettled(data) else { return false }
        projectGeneration &+= 1
        let previousProject = project
        project = data
        do { try applyEngineProject(data) }
        catch { project = previousProject; throw error }
        if !rack.routingProject.isEmpty { rack.routingProject = data }
        patternDataPublications &+= 1
        return true
    }

    /// True when a full publication would not (re)load any sample or clip: every sampler
    /// reference is loaded or loading, and every clip file has its slot. A reference
    /// that was missing is retried by republishing an unchanged project, so that
    /// publication must take the full path.
    @MainActor func mediaSettled(_ data:[String:Any]) -> Bool {
        for (index,item) in ProjectLayout.instruments(data).enumerated() {
            // Failed multisample zones drop their signature, so the next publication retries them.
            if !(item["zones"] as? [Any] ?? []).isEmpty, zoneSignatures[index] == nil { return false }
            guard item["type"] as? String == "sample" else { continue }
            let path = item["samplePath"] as? String ?? ""
            if path.isEmpty { if samples[index] != nil || pendingSamplePaths[index] != nil { return false }; continue }
            guard samples[index]?.path == path || pendingSamplePaths[index] == path else { return false }
        }
        for clip in AudioProject.clipAssetEntries(data) {
            guard let path = clip["path"] as? String ?? clip["samplePath"] as? String, !path.isEmpty else { continue }
            guard clipAssetSlots[path] != nil else { return false }
        }
        return true
    }

    @MainActor func applyProject(_ incoming: [String: Any],forcePluginState:Bool = false) throws {
        try requireInteractiveMutation()
        // Older documents and older UI payloads (tempo maps, built-in synths) are
        // brought to the current model first, exactly as ProjectStore does on open.
        let data = ProjectMigration.migrated(incoming)
        if !forcePluginState, try applyPatternDataOnly(data) { return }
        // Lock addresses/values must be validated even when a project uses the
        // legacy flat graph, before changing project or plug-in restore state.
        _ = try AudioProject.pluginStepFX(data)
        rack.notifyStateCaptureMutation()
        let changesRouting = data["routing"] != nil || rack.hasRouting
        if changesRouting { _ = try rack.validateRouting(data) }
        projectGeneration &+= 1
        if forcePluginState {
            projectIdentity &+= 1; previewGeneration &+= 1; playRequestGeneration &+= 1
            var stopPreview = st_default_step(); stopPreview.note = -1
            st_engine_preview_step(engine,0,stopPreview,nil,0,0.03)
            for task in directPluginLoads.values { task.cancel() }
            stagedSamples.removeAll()
        }
        // Strips for added tracks attach while the graph keeps running; they are
        // never removed, so a rollback below needs no graph change.
        try ensureChannelStrips(data)
        let previousProject = project
        project = data
        tempo = PayloadValue.number(data,"bpm",120)
        do {
            try syncClipAssets(AudioProject.clipAssetEntries(data))
            let applyAndReorder = {
                if changesRouting {
                    // Establish stable owner indices before interpreting insert
                    // slots and publish the graph with its C automation mapping.
                    try self.rack.configureRouting(data,commit:{
                        try self.applyEngineProject(data)
                    },rollback:{
                        try self.applyEngineProject(previousProject)
                    })
                }
                let reordered = try self.rack.restoreEffectOrder(self.pluginDescriptions(data),commit:{
                    try self.applyEngineProject(data)
                },rollback:{
                    self.project = previousProject; self.tempo = PayloadValue.number(previousProject,"bpm",120)
                    try self.applyEngineProject(previousProject)
                })
                if !reordered && !changesRouting { try self.applyEngineProject(data) }
            }
            if data["routing"] != nil { try applyAndReorder() }
            else {
                // A desired instance may still be loading while the old instance
                // occupies its slot. Publish exact-ID endpoints with the C project
                // in one quiescent transition; missing IDs deliberately stay inert.
                try rack.configureFlatStepFX(data,commit:applyAndReorder,rollback:{
                    self.project = previousProject; self.tempo = PayloadValue.number(previousProject,"bpm",120)
                    try self.applyEngineProject(previousProject)
                })
            }
        } catch {
            project = previousProject; tempo = PayloadValue.number(previousProject,"bpm",120)
            if changesRouting {
                do { try rack.configureRouting(previousProject,commit:{ try self.applyEngineProject(previousProject) }) }
                catch { emit("audio.error",["error":"The previous routing graph could not be restored: "+error.localizedDescription]) }
            }
            throw error
        }
        if let nextID = data["id"] as? String, let previousID = previousProject["id"] as? String, nextID != previousID {
            inputArmGeneration &+= 1; cancelMIDICaptures(reason:"session changed")
            noteRecordArmed = false; publishRecordingIntent()
        }
        let instruments = ProjectLayout.instruments(data)
        for (index,item) in instruments.enumerated() {
            if item["type"] as? String == "sample", let path = item["samplePath"] as? String, BuiltinSamples.isBuiltin(path), samples[index]?.path != path {
                // Built-in samples are small app content: load them at once, so a
                // migrated synth sounds on the very next note like it used to.
                sampleTasks[index]?.cancel(); pendingSamplePaths.removeValue(forKey:index)
                do { try installSample(try builtinSample(path),instrument:index) }
                catch { samples.removeValue(forKey:index); emit("sample.error",["instrument":index,"path":path,"error":error.localizedDescription]) }
            } else if item["type"] as? String == "sample", let path = item["samplePath"] as? String, let staged = stagedSamples.removeValue(forKey:path),
                      samples[index]?.path != path {
                sampleTasks[index]?.cancel(); pendingSamplePaths.removeValue(forKey:index)
                do { try installSample(staged,instrument:index) }
                catch { samples.removeValue(forKey:index); emit("sample.error",["instrument":index,"path":path,"error":error.localizedDescription]) }
            } else if item["type"] as? String == "sample", let path = item["samplePath"] as? String, !path.isEmpty, samples[index]?.path != path, pendingSamplePaths[index] != path {
                pendingSamplePaths[index] = path
                sampleTasks[index]?.cancel()
                samples.removeValue(forKey:index)
                _ = st_engine_set_sample(engine,Int32(index),nil,nil,0,sampleRate)
                sampleTasks[index] = Task { @MainActor [weak self] in
                    guard let self else { return }
                    do {
                        // The waveform overview is computed with the decode, off the main thread:
                        // a kit publishes many samples at once.
                        let (decoded,info) = try await Task.detached(priority:.userInitiated) { () throws -> (DecodedSample,[String:Any]) in
                            let sample = try AudioSamples.decode(path); return (sample,sample.info())
                        }.value
                        guard !Task.isCancelled, self.pendingSamplePaths[index] == path else { return }
                        try self.installSample(decoded,instrument:index)
                        self.pendingSamplePaths.removeValue(forKey:index)
                        self.emit("sample.loaded",["instrument":index,"sample":info])
                    } catch {
                        guard !Task.isCancelled, self.pendingSamplePaths[index] == path else { return }
                        self.pendingSamplePaths.removeValue(forKey:index)
                        self.samples.removeValue(forKey:index)
                        _ = st_engine_set_sample(self.engine,Int32(index),nil,nil,0,self.sampleRate)
                        self.emit("sample.error",["instrument":index,"path":path,"error":error.localizedDescription])
                    }
                }
            } else if item["type"] as? String == "sample", (item["samplePath"] as? String ?? "").isEmpty, samples[index] != nil || pendingSamplePaths[index] != nil {
                sampleTasks[index]?.cancel(); pendingSamplePaths.removeValue(forKey:index); samples.removeValue(forKey:index)
                _ = st_engine_set_sample(engine,Int32(index),nil,nil,0,sampleRate)
            }
            scheduleZones(item["zones"] as? [[String:Any]] ?? [],instrument:index)
        }
        let removed = Set(samples.keys).union(zoneSignatures.keys).union(pendingSamplePaths.keys).filter { $0 >= instruments.count }.sorted()
        if !removed.isEmpty {
            st_engine_begin_update(engine)
            for index in removed {
                sampleTasks[index]?.cancel(); zoneTasks[index]?.cancel(); pendingSamplePaths.removeValue(forKey:index)
                samples.removeValue(forKey:index); zoneSignatures.removeValue(forKey:index)
                _ = st_engine_set_sample(engine,Int32(index),nil,nil,0,sampleRate)
                st_engine_clear_zones(engine,Int32(index))
            }
            st_engine_commit_update(engine)
        }
        refreshMIDIRoutes()
        updatePluginMix()
        let previousLineInputs=lineInputSignature
        do {
            try synchronizeLineInputs(data)
            // A newly loaded route may have resolved against an input format that is still settling.
            if lineInputSignature != previousLineInputs { recheckLineInputsSoon() }
        }
        catch {
            let failure=error.localizedDescription
            var inactive=data
            inactive["tracks"]=((data["tracks"] as? [[String:Any]]) ?? []).map { item -> [String:Any] in
                var item=item;item.removeValue(forKey:"audioInput");return item
            }
            try? synchronizeLineInputs(inactive,force:true,report:false)
            // Reported once; later edits retrying the same failure stay quiet.
            lineInputError=failure
            if let message=lineInputReporter.update(failure) { emit("audio.error",["error":message]) }
        }
        let plugins = pluginDescriptions(data)
        let signature = pluginSignature(plugins)
        if signature != appliedPluginSignature || forcePluginState {
            appliedPluginSignature = signature
            restoreTask?.cancel()
            pluginRestoreRevision &+= 1
            let revision = pluginRestoreRevision
            pendingPluginRestore = revision
            restoreTask = Task { [weak self] in
                guard let self else { return }
                defer { if self.pendingPluginRestore == revision { self.pendingPluginRestore = nil } }
                do {
                    try await self.rack.restore(plugins,forceStates:forcePluginState)
                    guard self.pendingPluginRestore == revision else { return }
                    try self.refreshRoutingProject()
                    self.pendingPluginRestore = nil
                    self.updatePluginMix(); self.emit("plugins.changed",["plugins":self.pluginSnapshot(fresh:false)])
                }
                catch is CancellationError { }
                catch { self.emit("plugin.error",["error":error.localizedDescription]) }
            }
        }
        unavailableProject = nil; routingFailure = nil
    }

    /// An explicit session replacement may open an unsupported saved route for
    /// repair. Preserve its exact desired descriptors for saving, while the prior
    /// project's audio and asynchronous restoration are stopped rather than reused.
    @MainActor func retainUnavailableRouting(_ data:[String:Any],error:Error) async throws {
        try requireSessionReplacementAllowed()
        rack.notifyStateCaptureMutation()
        projectGeneration &+= 1; projectIdentity &+= 1; previewGeneration &+= 1; playRequestGeneration &+= 1
        let identity = projectIdentity,generation = projectGeneration
        pluginRestoreRevision &+= 1; pendingPluginRestore = nil
        // Publish the desired failed session before cancellation can suspend. In
        // that interval snapshots must retain this project, and old MIDI/play
        // requests must remain blocked even if a vendor takes time to return.
        unavailableProject = data; routingFailure = error.localizedDescription
        inputArmGeneration &+= 1; cancelMIDICaptures(reason:"session changed")
            noteRecordArmed = false; publishRecordingIntent()
        appliedPluginSignature = ""
        let previousRestore = restoreTask
        previousRestore?.cancel()
        previewPlayer?.stop(fade: false); graph.stop()
        panicExternal(); st_engine_stop(engine); st_engine_panic(engine)
        await previousRestore?.value
        guard !closed, identity == projectIdentity, generation == projectGeneration else {
            throw AudioSampleError.invalid("The unavailable routing project was superseded by a newer project.")
        }
        restoreTask = nil
        for task in sampleTasks.values { task.cancel() }
        for task in zoneTasks.values { task.cancel() }
        for task in clipTasks.values { task.cancel() }
        sampleTasks.removeAll(); zoneTasks.removeAll(); clipTasks.removeAll(); pendingSamplePaths.removeAll()
        previewPlayer?.stop(fade: false); graph.stop()
        try rack.awaitRenderQuiescence()
        panicExternal(); st_engine_stop(engine); st_engine_panic(engine)
        // Cancelled media decodes must be retryable when a valid project repairs
        // this session. Clear their slot reservations and matching native assets.
        st_engine_begin_update(engine)
        for index in zoneSignatures.keys { st_engine_clear_zones(engine,Int32(index)) }
        for slot in clipAssetSlots.values { _ = st_engine_set_audio_asset(engine,Int32(slot),nil,nil,0,sampleRate) }
        st_engine_commit_update(engine)
        zoneSignatures.removeAll(); clipAssetSlots.removeAll()
        _ = st_engine_capture_transport(engine)
        meters.reset(); renderCacheNeedsReset = true
        emit("routing.error",["error":error.localizedDescription,"projectUnavailable":true])
        emit("audio.error",["error":"The project was opened with unavailable routing. Its saved routing and plug-in state are preserved; repair the routing before playback. "+error.localizedDescription])
    }

    func routingStatus()->[String:Any] {
        var value = rack.routingStatus()
        if let routingFailure { value["enabled"] = false; value["projectUnavailable"] = true; value["error"] = routingFailure }
        return value
    }

    func routingCapabilities()->[String:Any] {
        var value = rack.routingCapabilities()
        if let routingFailure {
            value["projectUnavailable"] = true; value["error"] = routingFailure; value["fullGraphStems"] = false
            value["exportTargets"] = (value["exportTargets"] as? [[String:Any]] ?? []).map { target in
                var target = target; target["available"] = false; return target
            }
            if var sidechains = value["sidechains"] as? [String:Any] {
                sidechains["inputs"] = (sidechains["inputs"] as? [[String:Any]] ?? []).map { input in
                    var input = input; input["available"] = false; return input
                }
                value["sidechains"] = sidechains
            }
        }
        return value
    }

    /// Captures this restoration operation even if a later project supersedes it.
    /// Internal consumers can await its completion without confusing two generations.
    @MainActor func pluginRestorationBarrier() -> Task<Void,Never>? { restoreTask }

    @MainActor func awaitPluginRestoration() async {
        while let task = restoreTask {
            let revision = pluginRestoreRevision
            await task.value
            if revision == pluginRestoreRevision { return }
        }
    }

    @MainActor func retryRecoveredPlugins(_ payload:[String:Any]) async throws -> [String:Any] {
        guard let store = recoveryStore else { return rack.recoveryReport() }
        guard let keys = payload["keys"] as? [String], !keys.isEmpty else {
            throw AudioSampleError.invalid("Choose the plug-ins to retry from the recovery report.")
        }
        graphMutationsInFlight += 1
        defer { graphMutationsInFlight -= 1 }
        let identity = projectIdentity
        await awaitPluginRestoration()
        try requireInteractiveMutation()
        guard identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in retry was superseded by another project.") }
        try store.authorizeRetry(keys)
        let authorized = store.recoveryReport().suspects.filter { keys.contains($0.identity.key) && $0.retryAuthorized }
        let desired = pluginDescriptions(project).filter { item in
            authorized.contains { suspect in
                suspect.identity.componentID == item["component"] as? String &&
                (suspect.identity.format == .audioUnit || item["bundlePath"] == nil ||
                 suspect.identity.bundlePath == item["bundlePath"] as? String)
            }
        }
        var failures:[[String:Any]] = []
        var failedRecoveryKeys = Set<String>()
        for descriptor in desired {
            try requireInteractiveMutation()
            guard identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in retry was superseded by another project.") }
            // Re-read the desired slot after each await so an unrelated retry cannot
            // resurrect a slot removed or replaced while its predecessor was loading.
            guard let current = pluginDescriptions(project).first(where:{pluginKey($0) == pluginKey(descriptor)}),
                  NSDictionary(dictionary:current).isEqual(to:descriptor) else { continue }
            var request = descriptor
            if (request["component"] as? String ?? "").lowercased().hasPrefix("vst3:"), request["bundlePath"] == nil {
                let matches = authorized.filter { $0.identity.componentID == request["component"] as? String }
                guard matches.count == 1, let path = matches.first?.identity.bundlePath else {
                    failedRecoveryKeys.formUnion(matches.map { $0.identity.key })
                    failures.append(["name":descriptor["name"] as? String ?? "VST3 plug-in",
                        "message":"The saved slot has no bundle path and more than one recovery identity matches. Select one exact plug-in identity to retry."])
                    continue
                }
                request["bundlePath"] = path
            }
            let loadID = UUID()
            let task = Task { @MainActor in try await self.rack.load(request) }
            directPluginLoads[loadID] = task
            do {
                let result = try await task.value
                directPluginLoads.removeValue(forKey:loadID)
                try requireInteractiveMutation()
                guard identity == projectIdentity else { throw CancellationError() }
                if let active = result as? [String:Any] {
                    var updated = storedPluginDescriptions(project)
                    if let slot = updated.firstIndex(where:{pluginKey($0) == pluginKey(descriptor)}),
                       NSDictionary(dictionary:updated[slot]).isEqual(to:descriptor) { updated[slot] = active }
                    project["audioPlugins"] = updated
                    try refreshRoutingProject()
                }
            } catch {
                directPluginLoads.removeValue(forKey:loadID)
                guard !closed, identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in retry was superseded by another project.") }
                failedRecoveryKeys.formUnion(authorized.filter { $0.identity.componentID == descriptor["component"] as? String }.map { $0.identity.key })
                failures.append(["name":descriptor["name"] as? String ?? descriptor["component"] as? String ?? "Plug-in", "message":error.localizedDescription])
            }
        }
        if !failedRecoveryKeys.isEmpty { try store.acknowledge(Array(failedRecoveryKeys)) }
        appliedPluginSignature = ""
        updatePluginMix()
        var report = rack.recoveryReport()
        if !failures.isEmpty { report["retryErrors"] = failures }
        emit("plugins.changed",["plugins":pluginSnapshot()])
        return report
    }

    @MainActor func awaitProjectAssets() async {
        while !closed {
            let generation = projectGeneration
            await awaitPluginRestoration()
            for task in Array(sampleTasks.values) { await task.value }
            for task in Array(zoneTasks.values) { await task.value }
            for task in Array(clipTasks.values) { await task.value }
            if generation == projectGeneration { return }
        }
    }

    func requireInteractiveMutation() throws {
        guard !closed else { throw AudioSampleError.invalid("The audio engine is closed.") }
        guard !exporting else { throw AudioSampleError.invalid("Wait for export to finish before changing the audio graph or samples.") }
    }

    /// Bridge calls this before changing project-store/autosave identity. Export
    /// cleanup still owns the current session until its transport checkpoint is restored.
    @MainActor func requireSessionReplacementAllowed() throws {
        try requireInteractiveMutation()
        guard !freezeTrackRuntime.busy else {
            throw AudioSampleError.invalid("Wait for track Freeze or Unfreeze to finish before replacing the session.")
        }
        guard graphMutationsInFlight == 0 else {
            throw AudioSampleError.invalid("Wait for the current audio graph change to finish before replacing the session.")
        }
    }

    @MainActor func refreshRoutingProject() throws {
        if project["routing"] != nil {
            try rack.configureRouting(project,commit:{
                try self.applyEngineProject(self.project)
            })
        } else {
            try rack.configureFlatStepFX(project,commit:{
                if self.rack.hasRouting {
                    try self.rack.configureRouting(self.project,commit:{
                        try self.applyEngineProject(self.project)
                    })
                } else { try self.applyEngineProject(self.project) }
            })
        }
    }
}
