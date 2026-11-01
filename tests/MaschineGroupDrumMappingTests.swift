import Foundation
import TrackerEngine

@main struct MaschineGroupDrumMappingTests {
    static var checks = 0
    static func expect(_ yes: @autoclosure () -> Bool, _ message: String) {
        checks += 1; if !yes() { fatalError(message) }
    }
    static func raw(_ n: Int, _ engine: Int = 0) -> [Double] { var p = [Double](repeating: 0.5, count: n); p[0] = Double(engine); return p }
    static func physical(_ result: [String:Any], _ parameter: Int) -> Double {
        let model = st_drum_model_for_id(result["drumModel"] as! String)
        return st_drum_parameter_from_normalized(model, UInt32(parameter), (result["drumParameters"] as! [Double])[parameter])
    }
    static func close(_ actual: Double, _ expected: Double, _ message: String) { expect(abs(actual - expected) < 0.00001, message) }
    static func main() {
        let shapes = [("Kick",2,31,"kick"),("Snare",1,22,"snare"),("Hihat",2,18,"hihat"),("Tom",1,23,"tom"),("Perc",1,29,"perc"),("Cymbal",0,13,"cymbal")]
        for (type, version, count, model) in shapes {
            let result = MaschineGroupDrumMapping.map(type:"NI::MASCHINE::DATA::" + type,version:version,parameters:raw(count))!
            expect(result["drumModel"] as? String == model, "family identification")
            expect((result["drumParameters"] as! [Double]).count == 8, "exact target controls")
            let defaults = st_drum_model_for_id(model)
            for p in [1,2,4,5,6,7] { close((result["drumParameters"] as! [Double])[p],st_drum_parameter_default(defaults,UInt32(p)),"unverified fields keep CABI defaults") }
            expect((result["warnings"] as! [String]).contains(where: {$0.contains("millisecond")}),"no implicit percent-to-ms mapping")
            expect(MaschineGroupDrumMapping.map(type:type,version:version+1,parameters:raw(count)) == nil,"future version rejected")
            expect(MaschineGroupDrumMapping.map(type:type,version:version,parameters:raw(count-1)) == nil,"wrong array shape rejected")
        }
        var kick=raw(31,1);kick[1]=0.75;kick[2]=1;kick[3]=0.13;kick[4]=0.81
        let k=MaschineGroupDrumMapping.map(type:"Kick",version:2,parameters:kick)!
        close(physical(k,0),6,"Tune source physical semitones converted to target range")
        close(physical(k,3),81,"Impact uses serializer slot4, never Punch slot3")
        close(physical(k,1),350,"percentage Decay cannot overwrite physical-ms default")
        kick[0]=7;kick[1]=1
        close(physical(MaschineGroupDrumMapping.map(type:"Kick",version:2,parameters:kick)!,0),6,"acoustic Kick narrower octave span")
        for engine in [2,6,8,9] {kick[0]=Double(engine);let m=MaschineGroupDrumMapping.map(type:"Kick",version:2,parameters:kick)!;close(physical(m,0),0,"unknown/percentage Tune is not treated as semitones")}
        kick[0]=0
        close(physical(MaschineGroupDrumMapping.map(type:"Kick",version:2,parameters:kick)!,3),50,"Sub's inactive Impact not imported")
        var snare=raw(22,5);snare[1]=0;snare[4]=0.27
        let s=MaschineGroupDrumMapping.map(type:"Snare",version:1,parameters:snare)!
        close(physical(s,0),-9,"Snare18st range")
        close(physical(s,3),27,"Snare active Impact")
        for engine in [0,1,8,9] {snare[0]=Double(engine);close(physical(MaschineGroupDrumMapping.map(type:"Snare",version:1,parameters:snare)!,3),50,"inactive Snare Impact ignored")}
        var tom=raw(23,3);tom[1]=1
        close(physical(MaschineGroupDrumMapping.map(type:"Tom",version:1,parameters:tom)!,0),7,"Tom14st range")
        var perc=raw(29);perc[1]=0;perc[4]=0.14;perc[25]=0.29;perc[26]=0.83
        let p=MaschineGroupDrumMapping.map(type:"Perc",version:1,parameters:perc)!
        close(physical(p,0),-24,"Perc48st range")
        close(physical(p,3),83,"late-version Perc Impact index26, not Filter or Decay")
        var cymbal=raw(13);cymbal[3]=0.42
        close(physical(MaschineGroupDrumMapping.map(type:"Cymbal",version:0,parameters:cymbal)!,3),42,"Cymbal Impact slot3")
        kick=raw(31,99)
        let unknown=MaschineGroupDrumMapping.map(type:"Kick",version:2,parameters:kick)!
        expect((unknown["mappedParameters"] as! [Int]).isEmpty,"unknown engines retain every default")
        expect((unknown["warnings"] as! [String]).contains(where:{$0.contains("Unknown original engine")}),"unknown engine warning")
        for bad in [Double.nan,Double.infinity,-Double.infinity] {kick=raw(31);kick[6]=bad;expect(MaschineGroupDrumMapping.map(type:"Kick",version:2,parameters:kick) == nil,"nonfinite unrelated raw field rejected")}
        for bad in [-1.0,0.5,Double(Int32.max)+1] {kick=raw(31);kick[0]=bad;expect(MaschineGroupDrumMapping.map(type:"Kick",version:2,parameters:kick) == nil,"invalid engine scalar rejected")}
        kick=raw(31,1);kick[1]=2;kick[4] = -0.1
        let invalid=MaschineGroupDrumMapping.map(type:"Kick",version:2,parameters:kick)!
        close(physical(invalid,0),0,"out-of-range Tune default")
        close(physical(invalid,3),50,"out-of-range Impact default")
        expect((invalid["warnings"] as! [String]).filter({$0.contains("verified range")}).count==2,"out-of-range controls disclosed")
        expect(MaschineGroupDrumMapping.map(type:"Sampler",version:0,parameters:[]) == nil,"unrelated modules not synthesized")
        print("Maschine drum mapping: \(checks) checks passed; pure metadata only, no audio/vendor execution.")
    }
}
