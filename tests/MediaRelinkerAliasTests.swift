import Foundation

@main struct MediaRelinkerAliasTests {
    struct Failure: Error, CustomStringConvertible { let description: String }
    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws { if try !condition() {throw Failure(description:message)} }
    static func json(_ value: Any) throws -> Data {try JSONSerialization.data(withJSONObject:value,options:.sortedKeys)}
    static func fixture(_ source: String) -> [String:Any] {
        ["version":1,"bpm":120.0,"tracks":[["id":0]],"instruments":[["id":0,"zones":[["path":source,"samplePath":source,"rootNote":60]]]],"patterns":[["id":0,"length":4,"steps":[[]]]],"sequence":[0],
         "audioClips":[["id":"clip","track":0,"startBeat":0.0,"durationBeats":4.0,"path":source,"samplePath":source]]]
    }
    static func replacement(_ id:String,_ from:String,_ to:String)->[String:Any]{["id":id,"from":from,"to":to]}
    static func main() throws {
        let root=FileManager.default.temporaryDirectory.appendingPathComponent("ST-Relink-alias-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at:root,withIntermediateDirectories:true);defer{try? FileManager.default.removeItem(at:root)}
        let missing=root.appendingPathComponent("missing.wav").path,replacementFile=root.appendingPathComponent("replacement.wav"),other=root.appendingPathComponent("other.wav")
        try Data([1,2,3]).write(to:replacementFile);try Data([3,2,1]).write(to:other)
        let project=fixture(missing),before=try json(project)
        var failures=0
        for kind in ["zone","clip"] {
            do {
                let prefix=kind == "zone" ? "/instruments/0/zones/0/" : "/audioClips/0/"
                let result=try MediaRelinker.apply(project,selections:[replacement(prefix+"samplePath",missing,replacementFile.path)])
                let updated=result["project"] as! [String:Any]
                let item=kind == "zone" ? ((updated["instruments"] as! [[String:Any]])[0]["zones"] as! [[String:Any]])[0] : (updated["audioClips"] as! [[String:Any]])[0]
                try check(item["path"] as? String == replacementFile.path && item["samplePath"] as? String == replacementFile.path,"Relinking the samplePath alias must also repair the path actually played by the host")
                try check(try json(project) == before,"Relinking must not mutate the input project or its Undo snapshot")
                var rejected=false
                do{_=try MediaRelinker.apply(project,selections:[replacement(prefix+"path",missing,replacementFile.path),replacement(prefix+"samplePath",missing,other.path)])}catch{rejected=true}
                try check(rejected,"Linked aliases must reject conflicting replacement files atomically")
                var divergent=project
                if kind == "zone" {var instruments=divergent["instruments"] as! [[String:Any]];var zones=instruments[0]["zones"] as! [[String:Any]];zones[0]["samplePath"]=missing+"-different";instruments[0]["zones"]=zones;divergent["instruments"]=instruments}
                else {var clips=divergent["audioClips"] as! [[String:Any]];clips[0]["samplePath"]=missing+"-different";divergent["audioClips"]=clips}
                rejected=false
                do{_=try MediaRelinker.apply(divergent,selections:[replacement(prefix+"path",missing,replacementFile.path)])}catch{rejected=true}
                try check(rejected,"Already divergent aliases must never be silently merged")
                print("PASS \(kind) relink aliases, conflict rejection and original preservation")
            }catch{failures+=1;print("FAIL \(kind): \(error)")}
        }
        if failures>0 {exit(1)}
    }
}
