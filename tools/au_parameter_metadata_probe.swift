// Opt-in installed Audio Unit metadata check. No engine, device or audio output.
import Foundation
import AVFoundation
import AudioToolbox

@main struct AUParameterMetadataProbe {
    @MainActor static func main() async {
        let arguments=CommandLine.arguments
        guard arguments.count==3 else { fputs("Usage: probe <installed AU name> <report.json>\n",stderr); exit(2) }
        let requested=arguments[1]
        var report:[String:Any]=["requested":requested,"passed":false,"audioOutput":false]
        do {
            let search=AudioComponentDescription(componentType:0,componentSubType:0,componentManufacturer:0,componentFlags:0,componentFlagsMask:0)
            let components=AVAudioUnitComponentManager.shared().components(matching:search)
            let matches=components.filter { $0.name.caseInsensitiveCompare(requested) == .orderedSame }
            guard matches.count==1,let component=matches.first else {
                throw NSError(domain:"AUParameterMetadataProbe",code:1,userInfo:[NSLocalizedDescriptionKey:matches.isEmpty ? "No installed Audio Unit matches the exact name: \(requested)" : "The name matches multiple Audio Units; choose an unambiguous component name."])
            }
            report["name"]=component.name
            let node:AVAudioUnit=try await withCheckedThrowingContinuation { continuation in
                AVAudioUnit.instantiate(with:component.audioComponentDescription,options:[]) { node,error in
                    if let node { continuation.resume(returning:node) }
                    else { continuation.resume(throwing:error ?? NSError(domain:"AUInstantiation",code:1)) }
                }
            }
            let parameters=node.auAudioUnit.parameterTree?.allParameters ?? []
            for parameter in parameters {
                _ = AudioPluginParameterMetadata.info(parameter)
                _ = AudioPluginParameterMetadata.display(parameter)
            }
            report["parameterCount"]=parameters.count
            report["passed"]=true
        } catch { report["error"]=error.localizedDescription }
        do {
            let data=try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys])
            try data.write(to:URL(fileURLWithPath:arguments[2]))
        } catch { fputs("Cannot write probe report: \(error)\n",stderr); exit(2) }
        exit(report["passed"] as? Bool == true ? 0 : 1)
    }
}
