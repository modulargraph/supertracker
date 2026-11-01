import Foundation

extension MaschineGroupArchive {
    func channel(for sound:Sound) throws -> [String:Double] {
        let range = sound.offset..<sound.nameOffset
        let fields:[(String,Int,Bool)] = modern
            ? [("level",26,true),("pan",30,true),("mute",34,false),("muteAudio",38,false),("aux1",43,true),("aux2",53,true),("chokeGroup",77,false),("chokeMode",82,false)]
            : [("level",21,true),("pan",24,true),("mute",27,false),("muteAudio",30,false),("aux1",34,true),("aux2",42,true),("chokeGroup",61,false),("chokeMode",65,false)]
        var result:[String:Double] = [:]
        for (name,delta,float) in fields {result[name]=try scalar(finalID:sound.objectID+delta+soundLayout.channelShift,float:float,within:range)}
        guard (0...1).contains(result["level"]!),(-1...1).contains(result["pan"]!),
              (0...16).contains(result["chokeGroup"]!),[0,1].contains(result["mute"]!),[0,1].contains(result["muteAudio"]!) else {
            throw Invalid(message:"Sound mixer controls are outside their supported range.")
        }
        return result
    }
}
