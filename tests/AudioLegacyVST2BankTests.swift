import Foundation
@main struct BankTests {
 static func main() throws {
  let id:UInt32=0x4E694D61
  func packet(_ words:[UInt32],payload:Data=Data([0,1,128,255]))->Data {
   var value=Data("STV2".utf8)
   for word in words { for byte in 0..<4 { value.append(UInt8(truncatingIfNeeded:word>>(8*byte))) } }
   value.append(payload);return value
  }
  func encode(_ bytes:Data,id:String="vst2:4E694D61",version:Any=1)->String {
   try! PropertyListSerialization.data(fromPropertyList:["format":"SuperTracker.VST2","version":version,"id":id,"data":bytes],format:.binary,options:0).base64EncodedString()
  }
  var checks=0
  let valid=packet([1,id,1,2,4]);let bank=try AudioLegacyVST2Bank(component:"vst2:4E694D61",state:encode(valid))
  precondition(bank.uniqueID == id && bank.program == 2 && bank.payload == Data([0,1,128,255]));checks += 1
  for bad in [packet([2,id,1,2,4]),packet([1,id+1,1,2,4]),packet([1,id,0,2,4]),packet([1,id,2,2,4]),packet([1,id,1,UInt32.max,4]),packet([1,id,1,2,3]),Data(valid.prefix(23)),valid+Data([0])] {
   do { _ = try AudioLegacyVST2Bank(component:"vst2:4E694D61",state:encode(bad));fatalError("malformed packet accepted") } catch { checks += 1 }
  }
  for text in [encode(valid,id:"vst2:4E695236"),encode(valid,version:true),"notbase64"] {
   do { _ = try AudioLegacyVST2Bank(component:"vst2:4E694D61",state:text);fatalError("malformed binding accepted") } catch { checks += 1 }
  }
  let report:[String:Any] = ["passed":true,"checks":checks,"vendorExecution":false,"audio":false]
  print(String(data:try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]),encoding:.utf8)!)
 }
}
