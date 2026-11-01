import Foundation
import AVFoundation

@main enum PluginParameterMetadataTests {
    static func main() throws {
        var checks:[String]=[]
        func require(_ ok:Bool,_ message:String) throws { if !ok { throw NSError(domain:"PluginParameterMetadataTests",code:1,userInfo:[NSLocalizedDescriptionKey:message]) } }
        func parameter(_ min:Float=0,_ max:Float=1,_ unit:AudioUnitParameterUnit = .generic,_ labels:[String]?=nil) -> AUParameter {
            AUParameterTree.createParameter(withIdentifier:"original",name:"Original",address:42,min:min,max:max,unit:unit,unitName:nil,
                flags:[.flag_IsReadable,.flag_IsWritable],valueStrings:labels,dependentParameters:nil)
        }
        func count(_ p:AUParameter,_ expected:Int?,override:Int?=nil) throws {
            let info=AudioPluginParameterMetadata.info(p,explicitStepCount:override)
            try require(info["stepCount"] as? Int == expected,"Wrong discrete interval count: \(info)")
            if let expected {try require(info["normalizedStep"] as? Double == 1/Double(expected),"Wrong normalized grid spacing")}
            else {try require(info["normalizedStep"] == nil,"Continuous parameter acquired a grid")}
        }
        let formatted=parameter(-100,100)
        let tree=AUParameterTree.createTree(withChildren:[formatted])
        var nilPointers=0,formattedValues:[Float]=[]
        tree.implementorStringFromValueCallback={ _, pointer in
            guard let pointer else { nilPointers += 1; return "INVALID NIL" }
            formattedValues.append(pointer.pointee)
            return "Value \(pointer.pointee)"
        }
        formatted.value=12.5
        let formattedInfo=AudioPluginParameterMetadata.info(formatted)
        try require(formattedInfo["display"] as? String == "Value 12.5","Metadata formatter did not receive the actual current value")
        formatted.value = -3.25
        try require(AudioPluginParameterMetadata.display(formatted) == "Value -3.25","Generic editor formatter did not receive the edited value")
        try require(nilPointers==0 && formattedValues == [12.5,-3.25],"AU formatter received a null or stale value pointer")
        checks.append("AU metadata and generic editor formatting always pass explicit current value pointers to vendor callbacks")

        let continuous=parameter(-100,100)
        continuous.value=Float(0.123456789)
        try count(continuous,nil)
        try require(AudioPluginParameterMetadata.grid(continuous)==nil,"Continuous control was quantized")
        let continuousInfo=AudioPluginParameterMetadata.info(continuous)
        try require(continuousInfo["value"] as? Float == continuous.value && continuousInfo["normalized"] as? Float == (continuous.value+100)/200,"Continuous readback changed")
        checks.append("Continuous parameters retain raw and normalized readback with no inferred grid")

        let binary=parameter(0,1,.boolean);try count(binary,1)
        let binaryGrid=AudioPluginParameterMetadata.grid(binary)!
        try require(binaryGrid.raw(0.499999,normalized:true)==0 && binaryGrid.raw(0.5,normalized:true)==1,"Binary threshold is incorrect")
        try require(binaryGrid.raw(-1,normalized:true)==0 && binaryGrid.raw(2,normalized:true)==1,"Binary endpoint clamping failed")
        try count(parameter(1,2,.boolean),nil)
        checks.append("Boolean endpoints and midpoint are snapped; unusual nonzero-minimum boolean domains are not guessed")

        let indexed=parameter(-2,2,.indexed,["A","B","C","D","E"]);try count(indexed,4)
        let indexedGrid=AudioPluginParameterMetadata.grid(indexed)!
        try require(indexedGrid.raw(-0.6,normalized:false) == -1 && indexedGrid.raw(-0.5,normalized:false)==0,"Bipolar indexed snapping used a zero-based raw domain")
        try require(indexedGrid.raw(0.75,normalized:true)==1 && indexedGrid.raw(1,normalized:true)==2,"Indexed normalized endpoints changed")
        try require(AudioPluginParameterMetadata.info(indexed)["valueStrings"] as? [String] == ["A","B","C","D","E"],"Indexed labels lost")
        checks.append("Bipolar indexed raw values, normalized fractions and matching labels agree")

        let offset=parameter(3,6,.indexed);try count(offset,3)
        let offsetGrid=AudioPluginParameterMetadata.grid(offset)!
        try require(offsetGrid.raw(0,normalized:true)==3 && offsetGrid.raw(0.5,normalized:true)==5 && offsetGrid.raw(1,normalized:true)==6,"Nonzero indexed minimum lost")
        try count(parameter(0.25,3.25,.indexed),nil)
        try count(parameter(4,4,.indexed),nil)
        checks.append("Nonzero indexed minima work; fractional indexed bounds and degenerate ranges stay unclassified")

        let labeled=parameter(2,4,.generic,["One","Two","Three"]);try count(labeled,2)
        let malformed=parameter(2,5,.generic,["One","Two","Three"]);try count(malformed,nil)
        try require(AudioPluginParameterMetadata.info(malformed)["valueStrings"]==nil,"Mismatched labels were published as a valid domain")
        checks.append("Value strings infer a grid only when they span the complete integer range")

        let vst=parameter()
        try count(vst,3,override:3)
        let grid=AudioPluginParameterMetadata.grid(vst,explicitStepCount:3)!
        try require(grid.raw(0.49,normalized:true)==Float(1.0/3) && grid.raw(0.5,normalized:true)==Float(2.0/3),"VST3 discrete normalized snapping failed")
        try require(vst.minValue==0 && vst.maxValue==1 && vst.unit == .generic,"VST3 raw domain changed to integer indices")
        try count(parameter(0,1,.boolean),nil,override:0)
        try count(vst,nil,override:-1)
        checks.append("VST3 stepCount uses normalized intervals; explicit continuous/unknown overrides suppress inference")

        // Simulate authoritative controller/state readback after restore. Metadata
        // must report it verbatim, even if the plug-in returns an off-grid value.
        for value:Float in [0,Float(1.0/3),Float(2.0/3),1,0.42] {
            vst.value=value
            let info=AudioPluginParameterMetadata.info(vst,explicitStepCount:3)
            try require(info["value"] as? Float == value && info["normalized"] as? Float == value,"Authoritative state/controller value was rounded during readback")
        }
        checks.append("Restored grid values and off-grid authoritative readback are reported without cosmetic rounding")

        for n in [1,2,3,7,127,1023,Int(Int32.max)] {
            let g=AudioPluginParameterMetadata.Grid(minimum:0,maximum:1,stepCount:n)!
            for value in [0.0,0.000001,0.125,0.5,0.999999,1.0] {
                let snapped=g.normalized(value)
                try require(snapped>=0 && snapped<=1 && abs(snapped*Double(n)-(snapped*Double(n)).rounded())<0.000001,"Grid point drift")
            }
        }
        try require(AudioPluginParameterMetadata.Grid(minimum:0,maximum:.infinity,stepCount:3)==nil,"Nonfinite bounds accepted")
        try require(AudioPluginParameterMetadata.Grid(minimum:0,maximum:1,stepCount:Int.max)==nil,"Unrepresentable interval count accepted")
        checks.append("Binary through Int32-max interval counts remain finite and bounded")
        let report:[String:Any]=["passed":true,"checks":checks,"scope":"AUParameter objects and arithmetic only; no AudioUnit instances, graph, device or vendor plug-in"]
        try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]).write(to:URL(fileURLWithPath:CommandLine.arguments[1]))
        print("PASS \(checks.count) plug-in parameter metadata groups")
    }
}
