import AudioToolbox

/// Shared lifecycle of SuperTracker's own plug-in format adapters (VST3, VST2).
/// The rack closes vendor code explicitly once rendering is quiesced.
protocol HostedPluginAdapter: AnyObject {
    func closePlugin()
    func captureRenderContext()
    func synchronizeParameters()
    var status: [String: Any] { get }
}

extension VST3AudioUnit: HostedPluginAdapter {}
extension VST2AudioUnit: HostedPluginAdapter {}
