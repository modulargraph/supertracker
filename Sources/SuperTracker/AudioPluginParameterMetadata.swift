import Foundation
import AVFoundation

/// Discrete metadata is control-side only. Readback and scheduled automation
/// remain in the plug-in's existing raw domain; only explicit edits are snapped.
enum AudioPluginParameterMetadata {
    struct Grid: Equatable {
        let minimum:Double, maximum:Double
        /// Number of intervals, not number of values (a switch has one interval).
        let stepCount:Int
        var normalizedStep:Double { 1 / Double(stepCount) }

        init?(minimum:Double,maximum:Double,stepCount:Int) {
            guard minimum.isFinite,maximum.isFinite,maximum>minimum,
                  (maximum-minimum).isFinite,stepCount>0,stepCount<=Int(Int32.max) else { return nil }
            self.minimum=minimum;self.maximum=maximum;self.stepCount=stepCount
        }
        func normalized(_ value:Double) -> Double {
            let clamped=max(0,min(1,value))
            return (clamped*Double(stepCount)).rounded(.toNearestOrAwayFromZero)/Double(stepCount)
        }
        func raw(_ value:Double,normalized inputIsNormalized:Bool) -> AUValue {
            let input=inputIsNormalized ? value : (value-minimum)/(maximum-minimum)
            return AUValue(minimum+normalized(input)*(maximum-minimum))
        }
    }

    /// An explicit zero means continuous/unknown and prevents AU-unit inference
    /// for normalized adapters. Invalid native indexed ranges are not guessed.
    static func grid(_ parameter:AUParameter,explicitStepCount:Int? = nil) -> Grid? {
        let minimum=Double(parameter.minValue),maximum=Double(parameter.maxValue)
        if let explicitStepCount { return Grid(minimum:minimum,maximum:maximum,stepCount:explicitStepCount) }
        if parameter.unit == .boolean {
            return minimum == 0 ? Grid(minimum:minimum,maximum:maximum,stepCount:1) : nil
        }
        let width=maximum-minimum
        guard minimum.isFinite,maximum.isFinite,minimum.rounded(.towardZero)==minimum,
              maximum.rounded(.towardZero)==maximum,width>0,width<=Double(Int32.max) else { return nil }
        // AU indexed values are integers. A label list is corroborating evidence
        // only when it describes that entire integer range, including both ends.
        let labels=parameter.valueStrings
        guard parameter.unit == .indexed || labels.map({$0.count>=2 && Double($0.count-1)==width}) == true else { return nil }
        return Grid(minimum:minimum,maximum:maximum,stepCount:Int(width))
    }

    /// AUv2 bridges forward this pointer to ParameterStringFromValue. Some
    /// plug-ins (including JUCE AUs) dereference it even though AUParameter
    /// permits nil to request the current value. Always supply a real value.
    static func display(_ parameter:AUParameter,value:AUValue? = nil) -> String {
        var current=value ?? parameter.value
        return parameter.string(fromValue:&current)
    }

    /// value: the plug-in's own current value, where the tree's cached one can lag (see liveParameterValues).
    static func info(_ parameter:AUParameter,explicitStepCount:Int? = nil,value live:AUValue? = nil) -> [String:Any] {
        let width=parameter.maxValue-parameter.minValue,value=live ?? parameter.value
        var result:[String:Any] = ["address":String(parameter.address),"id":parameter.identifier,
            "name":parameter.displayName,"min":parameter.minValue,"max":parameter.maxValue,
            "value":value,"normalized":width>0 ? (value-parameter.minValue)/width : 0,
            "unit":parameter.unit.rawValue,"unitName":parameter.unitName ?? "",
            "display":display(parameter,value:value),"flags":parameter.flags.rawValue,
            "writable":parameter.flags.contains(.flag_IsWritable),
            "automatable":!parameter.flags.contains(.flag_NonRealTime)]
        if let grid=grid(parameter,explicitStepCount:explicitStepCount) {
            result["stepCount"]=grid.stepCount;result["normalizedStep"]=grid.normalizedStep
            if let labels=parameter.valueStrings,labels.count==grid.stepCount+1 { result["valueStrings"]=labels }
        }
        return result
    }
}
