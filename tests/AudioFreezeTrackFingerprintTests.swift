import Foundation

@main enum AudioFreezeTrackFingerprintTests {
    typealias F = AudioFreezeTrackFingerprint
    static func main() throws {
        var checks = [String]()
        func check(_ ok: Bool, _ message: String) throws { if !ok { throw NSError(domain: message, code: 1) } }
        func record(_ name: String) { checks.append(name); print("PASS \(name)") }
        func fixture() -> [String: Any] {
            ["version":1,"bpm":120.0,"linesPerBeat":4,"swing":0.0,"sequence":[0],
             "tracks":[["id":0,"instrument":0,"name":"Source","volume":0.6]],
             "instruments":[["id":0,"type":"sample","samplePath":"/original/source.wav","rootNote":60,
                              "zones":[["path":"/original/zone.wav","samplePath":"/original/zone.wav","rootNote":62]]]],
             "audioPlugins":[["track":0,"kind":"effect","slot":0,"component":"test-au","instanceID":"effect-1","state":"opaque-original","bundlePath":"/vendor/Effect.component"]],
             "patterns":[["id":0,"length":4,"steps":[[["note":60,"instrument":0,"velocity":100],NSNull(),NSNull(),NSNull()]],"automation":[]]],
             "audioClips":[["id":"clip","track":0,"path":"/original/clip.wav","startBeat":0.0,"durationBeats":1.0,
                            "takeSet":["version":1,"takes":[["path":"/original/take.wav"]]],"takes":[["samplePath":"/original/old-take.wav"]]]]]
        }
        func frozen(_ project:[String:Any], fingerprint:String? = nil) throws -> [String:Any] {
            var result=project, tracks=project["tracks"] as! [[String:Any]]
            tracks[0]["frozen"]=["version":1,"id":"print-1","path":"/print.wav","sha256":"print-hash",
                                  "fingerprint":try fingerprint ?? F.fingerprint(project,index:0)]
            result["tracks"]=tracks;return result
        }
        func captured(_ project:[String:Any]) -> String { ((project["tracks"] as! [[String:Any]])[0]["frozen"] as! [String:Any])["fingerprint"] as! String }
        func fresh(_ project:[String:Any]) throws -> Bool { captured(project) == (try F.fingerprint(project,index:0)) }
        func moved(_ project:[String:Any],prefix:String) -> [String:Any] {
            var result=project,instruments=project["instruments"] as! [[String:Any]],clips=project["audioClips"] as! [[String:Any]]
            instruments[0]["samplePath"]=prefix+"/source.wav"
            instruments[0]["zones"]=[["path":prefix+"/zone.wav","samplePath":prefix+"/zone.wav","rootNote":62]]
            clips[0]["path"]=prefix+"/clip.wav";clips[0]["takeSet"]=["version":1,"takes":[["path":prefix+"/take.wav"]]]
            clips[0]["takes"]=[["samplePath":prefix+"/old-take.wav"]]
            result["instruments"]=instruments;result["audioClips"]=clips;return result
        }
        func aliases(_ from:String,_ to:String) -> [String:String] {
            Dictionary(uniqueKeysWithValues:["source","zone","clip","take","old-take"].map{(from+"/"+$0+".wav",to+"/"+$0+".wav")})
        }
        // Captured from the original v1 implementation before extraction.
        try check(try F.fingerprint(fixture(),index:0) == "5b05e64cba747654cf5c6c641b95b9d5979bd2ab7e659709a4415a1fed19b5df",
                  "Historical saved v1 fingerprint must remain compatible")
        record("historical v1 fingerprint stays byte-compatible")
        let source=try frozen(fixture()),portable=moved(source,prefix:"Session.media")
        try check(!(try fresh(portable)),"Original path-sensitive regression must reproduce")
        let fixed=try F.preservingRelocatedSources(from:source,to:portable,verifiedAliases:aliases("/original","Session.media"))
        try check(try fresh(fixed),"Verified sample/zone/clip/take aliases must preserve freshness")
        try check(captured(fixed) != captured(source),"Path spelling changed, so compatible new fingerprint is required")
        record("verified sample, zone, clip and take collection preserves freshness")
        let reopened=moved(fixed,prefix:"/moved/Session.media")
        let opened=try F.preservingRelocatedSources(from:fixed,to:reopened,verifiedAliases:aliases("Session.media","/moved/Session.media"))
        try check(try fresh(opened),"Package-relative expansion must preserve already verified identity")
        let repeated=try F.preservingRelocatedSources(from:opened,to:opened,verifiedAliases:aliases("/original","/unused"))
        try check(captured(repeated)==captured(opened),"Repeated save/no-op aliases must not alter fingerprint")
        record("portable reopen and repeated no-op preserve current fingerprint")
        let stale=try frozen(fixture(),fingerprint:String(repeating:"0",count:64))
        let staleResult=try F.preservingRelocatedSources(from:stale,to:moved(stale,prefix:"Session.media"),verifiedAliases:aliases("/original","Session.media"))
        try check(captured(staleResult)==captured(stale) && !(try fresh(staleResult)),"Stale records must never be recertified")
        record("already-stale records remain stale")
        for mode in ["missing","partial","wrong"] {
            var mapping=aliases("/original","Session.media")
            if mode=="missing" {mapping=[:]} else if mode=="partial" {mapping.removeValue(forKey:"/original/take.wav")} else {mapping["/original/source.wav"]="wrong.wav"}
            let result=try F.preservingRelocatedSources(from:source,to:portable,verifiedAliases:mapping)
            try check(captured(result)==captured(source) && !(try fresh(result)),"\(mode) proof must not certify moved source")
        }
        record("missing, partial and incorrect relocation proof never recertify")
        for mode in ["note","tempo","sampler","clip"] {
            var changed=portable
            if mode=="tempo" {changed["bpm"]=121.0}
            if mode=="note" {var p=changed["patterns"] as! [[String:Any]];p[0]["steps"]=[[["note":61,"instrument":0,"velocity":100],NSNull(),NSNull(),NSNull()]];changed["patterns"]=p}
            if mode=="sampler" {var i=changed["instruments"] as! [[String:Any]];i[0]["rootNote"]=61;changed["instruments"]=i}
            if mode=="clip" {var c=changed["audioClips"] as! [[String:Any]];c[0]["durationBeats"]=2.0;changed["audioClips"]=c}
            let result=try F.preservingRelocatedSources(from:source,to:changed,verifiedAliases:aliases("/original","Session.media"))
            try check(captured(result)==captured(source) && !(try fresh(result)),"Concurrent \(mode) source edit must stay stale")
        }
        record("simultaneous musical and source-setting edits cannot be hidden by relocation")
        var opaque=fixture();var instruments=opaque["instruments"] as! [[String:Any]]
        instruments[0]["extension"]=["path":"/original/source.wav"];opaque["instruments"]=instruments
        let opaqueSource=try frozen(opaque);var opaqueMoved=moved(opaqueSource,prefix:"Session.media")
        instruments=opaqueMoved["instruments"] as! [[String:Any]];instruments[0]["extension"]=["path":"Session.media/source.wav"];opaqueMoved["instruments"]=instruments
        let opaqueResult=try F.preservingRelocatedSources(from:opaqueSource,to:opaqueMoved,verifiedAliases:aliases("/original","Session.media"))
        try check(captured(opaqueResult)==captured(opaqueSource) && !(try fresh(opaqueResult)),"Unknown path-shaped extension must not be treated as source relocation")
        record("unknown extension paths stay semantically opaque")
        var pluginChanged=portable;var plugins=pluginChanged["audioPlugins"] as! [[String:Any]];plugins[0]["bundlePath"]="/other/Effect.component";pluginChanged["audioPlugins"]=plugins
        var proof=aliases("/original","Session.media");proof["/vendor/Effect.component"]="/other/Effect.component"
        let pluginResult=try F.preservingRelocatedSources(from:source,to:pluginChanged,verifiedAliases:proof)
        try check(captured(pluginResult)==captured(source) && !(try fresh(pluginResult)),"Plugin installation path is not media")
        record("plugin binding changes are never converted into media aliases")
        var replaced=portable;var tracks=replaced["tracks"] as! [[String:Any]];var raw=tracks[0]["frozen"] as! [String:Any];raw["id"]="another-print";tracks[0]["frozen"]=raw;replaced["tracks"]=tracks
        let replacedResult=try F.preservingRelocatedSources(from:source,to:replaced,verifiedAliases:aliases("/original","Session.media"))
        try check(captured(replacedResult)==captured(source) && !(try fresh(replacedResult)),"Another print must not inherit recertification")
        record("replacement print identity is fenced")
        var cosmetic=portable;tracks=cosmetic["tracks"] as! [[String:Any]];tracks[0]["volume"]=0.2;tracks[0]["name"]="Renamed";cosmetic["tracks"]=tracks
        let cosmeticResult=try F.preservingRelocatedSources(from:source,to:cosmetic,verifiedAliases:aliases("/original","Session.media"))
        try check(try fresh(cosmeticResult),"Already-live mix and cosmetic fields remain excluded")
        record("existing live mix/cosmetic semantics remain unchanged")
        if CommandLine.arguments.count>1 {
            let output=URL(fileURLWithPath:CommandLine.arguments[1]);try FileManager.default.createDirectory(at:output,withIntermediateDirectories:true)
            try JSONSerialization.data(withJSONObject:["passed":true,"checks":checks,"fixtureFingerprint":try F.fingerprint(fixture(),index:0),"audioExecuted":false],options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("report.json"))
        }
        print("PASS \(checks.count) pure fingerprint relocation groups")
    }
}
