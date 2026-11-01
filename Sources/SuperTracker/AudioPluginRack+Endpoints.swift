import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Render endpoint reads, retirement and diagnostics.
extension AudioPluginRack {
    /// The host brackets main-mixer pre/post render and each source callback. These
    /// scopes nest with endpoint scheduling scopes and never wait for a writer.
    /// A shared reader lease without access to rack endpoints or processors.
    var renderReadScope: AudioEndpointReadScope { endpointStore.readScope }

    @inline(__always) func beginRenderRead() { endpointStore.beginRead() }
    @inline(__always) func endRenderRead() { endpointStore.endRead() }

    /// Call only on the control thread, after graph detach transactions have ended.
    /// Zero readers proves completion of registered work; it is not inferred from
    /// pause(). New readers can use only current endpoints and attached graph nodes.
    @discardableResult func collectRetiredEndpoints() -> Int {
        precondition(Thread.isMainThread)
        guard endpointStore.activeReaders == 0 else { return 0 }
        for instance in retiredUnits {
            auStateReuse.remove(ObjectIdentifier(instance.node.auAudioUnit))
            instance.removeRenderObserver()
            if let adapter = instance.node.auAudioUnit as? HostedPluginAdapter {
                adapter.closePlugin(); instance.recovery?.vendorClosed = true
            }
        }
        retiredUnits.removeAll(keepingCapacity: true)
        let released = endpointStore.collectRetired() + routingRenderStore.collectRetired() + flatStepRenderStore.collectRetired()
        collectRecoveryRecords()
        return released
    }

    /// Read-only per-unit render diagnostics; never calls into vendor code.
    func renderDiagnostics() -> [[String: Any]] {
        instances.values.sorted { $0.instanceID < $1.instanceID }.map { item in
            ["instanceID": item.instanceID, "owner": item.ownerID as Any? ?? NSNull(),
             "track": item.target.track, "kind": item.target.kind, "slot": item.target.slot,
             "component": item.component.id, "name": item.component.name,
             "observerFailure": item.renderMonitor.failureContext() as Any? ?? NSNull()]
        }
    }

    func endpointLifetimeStatus() -> [String: Any] {
        ["activeReaders": endpointStore.activeReaders, "retiredEndpoints": endpointStore.retiredCount,
         "retiredUnits": retiredUnits.count, "activeInstances": instances.count]
    }

    /// Control-thread diagnostics for bounded state retention; never touches a AU.
    func auStateReuseStatistics() -> AudioAUStateReuse.Statistics { auStateReuse.statistics }
}
