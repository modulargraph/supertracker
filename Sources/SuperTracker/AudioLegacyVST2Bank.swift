import Foundation
import CoreFoundation

/// Exact extraction of our saved VST2 bank envelope. No vendor is instantiated
/// and no component is selected until this binding has been validated.
struct AudioLegacyVST2Bank {
    let uniqueID:UInt32
    let program:Int32
    let payload:Data

    init(component:String,state:String) throws {
        func invalid()->NSError { NSError(domain:"SuperTracker.VST2Migration",code:1,userInfo:[NSLocalizedDescriptionKey:"The saved VST2 bank does not match its exact component, version, program or payload size."]) }
        guard component.count == 13,component.lowercased().hasPrefix("vst2:"),
              let uniqueID=UInt32(component.dropFirst(5),radix:16),uniqueID != 0,
              state.utf8.count <= 48 * 1024 * 1024,
              let bytes=Data(base64Encoded:state),bytes.count <= 34 * 1024 * 1024,
              let document=(try? PropertyListSerialization.propertyList(from:bytes,options:[],format:nil)) as? [String:Any],
              document["format"] as? String == "SuperTracker.VST2",
              let version=document["version"] as? NSNumber,CFGetTypeID(version) != CFBooleanGetTypeID(),version.doubleValue == 1,
              (document["id"] as? String)?.uppercased() == component.uppercased(),
              let packet=document["data"] as? Data,packet.count > 24,packet.count <= 32 * 1024 * 1024 + 24,
              packet.prefix(4) == Data("STV2".utf8) else { throw invalid() }
        func word(_ offset:Int)->UInt32 { (0..<4).reduce(UInt32(0)) { $0 | UInt32(packet[offset+$1]) << ($1*8) } }
        guard word(4) == 1,word(8) == uniqueID,word(12) == 1,
              word(16) <= UInt32(Int32.max),Int(word(20)) == packet.count-24 else { throw invalid() }
        self.uniqueID=uniqueID;program=Int32(word(16));payload=Data(packet.dropFirst(24))
    }
}
