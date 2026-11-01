import Foundation
import TrackerEngine

/// Control-thread ledger for optional native input edges. This is the sole
/// consumer of the engine's input ACK ring; it never reads mutable render state.
/// Expiring an accepted edge only expires its recording result, never retries the
/// audible edge. Late ACKs are still drained to return their reserved capacity.
final class AudioInputCapture {
    static let capability:[String:Any] = ["version":1,"supported":true]
    struct Request {
        let owner:String
        let on:Bool
        let track:Int
        let instrument:Int
        let note:Int
        let velocity:Int
        let applyNote:Bool
        let context:[String:Any]
        let patternIDs:Set<Int>
    }
    private struct Pending {
        let request:Request
        let enqueuedAt:ContinuousClock.Instant
        let deadline:ContinuousClock.Instant
        let isCurrent:()->Bool
        let completion:([String:Any])->Void
    }
    private var nextID:UInt64 = 0
    private var pending:[UInt64:Pending] = [:]
    private var delivered = 0, expired = 0, invalidated = 0
    var diagnostics:[String:Any] { ["accepted":String(nextID),"pending":pending.count,"delivered":delivered,"expired":expired,"invalidated":invalidated] }
    private let engine:OpaquePointer
    init(engine:OpaquePointer) { self.engine = engine }

    /// Returns nil only if nothing was enqueued. The caller may perform exactly
    /// one legacy performance fallback in that case, never after acceptance.
    @discardableResult
    func enqueue(_ request:Request,isCurrent:@escaping ()->Bool,completion:@escaping ([String:Any])->Void) -> UInt64? {
        guard nextID < UInt64.max, pending.count < Int(ST_INPUT_ACK_CAPACITY) else {
            completion(unavailable(request,id:nil,reason:"The input recording queue is full.")); return nil
        }
        nextID += 1
        let id = nextID
        guard st_engine_input_note(engine,id,request.on ? Int32(ST_INPUT_NOTE_ON) : Int32(ST_INPUT_NOTE_OFF),
            Int32(clamping:request.track),Int32(clamping:request.instrument),Int32(clamping:request.note),
            Int32(clamping:request.velocity),request.applyNote ? 1 : 0) != 0 else {
            completion(unavailable(request,id:id,reason:"The audio engine could not accept this recording edge.")); return nil
        }
        let now=ContinuousClock.now
        pending[id] = Pending(request:request,enqueuedAt:now,deadline:now.advanced(by:.seconds(1)),isCurrent:isCurrent,completion:completion)
        return id
    }

    func poll() {
        var ack = STInputAck()
        // Capacity bounds even a producer that is active during this drain.
        for _ in 0..<Int(ST_INPUT_ACK_CAPACITY) {
            guard st_engine_pop_input_ack(engine,&ack) != 0 else { break }
            guard let item = pending.removeValue(forKey:ack.requestID) else { continue }
            guard item.isCurrent() else {
                invalidated += 1
                item.completion(unavailable(item.request,id:ack.requestID,reason:"The recording session or arm intent changed.")); continue
            }
            let request = item.request
            let matches = ack.edge == (request.on ? ST_INPUT_NOTE_ON : ST_INPUT_NOTE_OFF)
                && Int(ack.track) == request.track && Int(ack.instrument) == request.instrument
                && Int(ack.note) == request.note && Int(ack.velocity) == request.velocity
                && (ack.applyNote != 0) == request.applyNote
            guard matches else {
                item.completion(unavailable(request,id:ack.requestID,reason:"The native input edge did not match its request.")); continue
            }
            var result = base(request,id:ack.requestID)
            let applied = ack.status == ST_INPUT_ACK_APPLIED && request.patternIDs.contains(Int(ack.pattern))
            result["status"] = applied ? "applied" : ack.status == ST_INPUT_ACK_DISCONTINUITY ? "discontinuity" : ack.status == ST_INPUT_ACK_CONTROL_FLUSH ? "controlFlush" : "unavailable"
            result["applied"] = applied
            result["playing"] = applied && ack.flags & Int32(ST_INPUT_CLOCK_PLAYING) != 0
            result["recording"] = applied && ack.flags & Int32(ST_INPUT_CLOCK_RECORDING) != 0
            result["mapped"] = applied && ack.flags & Int32(ST_INPUT_CLOCK_MAPPED) != 0
            result["pattern"] = Int(ack.pattern); result["row"] = Int(ack.row); result["sequence"] = Int(ack.sequence); result["early"] = ack.early != 0
            result["renderFrame"] = String(ack.renderFrame)
            if ack.graphFrame != Int64.min { result["graphFrame"] = String(ack.graphFrame) }
            if applied && ack.flags & Int32(ST_INPUT_CLOCK_PLAYING) != 0 && ack.distanceBeats.isFinite {
                result["recordingClock"] = ["distanceBeats":ack.distanceBeats,"runID":String(ack.runID),"timingRevision":String(ack.timingRevision)]
            }
            item.completion(result)
            delivered += 1
        }
        let now = ContinuousClock.now
        for id in pending.keys.sorted() {
            guard let item = pending[id] else { continue }
            let current=item.isCurrent()
            guard now >= item.deadline || !current else { continue }
            pending.removeValue(forKey:id)
            if current { expired += 1 } else { invalidated += 1 }
            var result=unavailable(item.request,id:id,reason:current
                ? "The audio engine did not acknowledge the input edge in time."
                : "The recording session or arm intent changed before the input was applied.")
            let elapsed=item.enqueuedAt.duration(to:now).components
            result["waitMilliseconds"]=Double(elapsed.seconds)*1000+Double(elapsed.attoseconds)/1e15
            result["ownershipCurrent"]=current
            item.completion(result)
        }
    }

    func cancelAll(reason:String) {
        let cancelled = pending.sorted { $0.key < $1.key }; pending.removeAll()
        for (id,item) in cancelled { item.completion(unavailable(item.request,id:id,reason:reason)) }
    }

    private func base(_ request:Request,id:UInt64?) -> [String:Any] {
        var result:[String:Any] = ["version":1,"inputOwner":request.owner,"on":request.on,"track":request.track,
            "instrument":request.instrument,"note":request.note,"velocity":request.velocity,"applyNote":request.applyNote,
            "recordingContext":request.context]
        if let id { result["requestID"] = String(id) }
        return result
    }
    private func unavailable(_ request:Request,id:UInt64?,reason:String) -> [String:Any] {
        var result = base(request,id:id)
        result.merge(["status":"unavailable","applied":false,"playing":false,"recording":false,"mapped":false,"reason":reason]) { _,new in new }
        return result
    }

    /// JSON projection shared with STInputCapture.recordingContext(). Arrays
    /// preserve project order; musical content is excluded.
    static func recordingContext(_ project:[String:Any]) -> [String:Any] {
        func number(_ item:[String:Any],_ key:String,_ fallback:Double)->Double { PayloadValue.number(item,key,fallback) }
        func integer(_ value:Double)->Int { guard value.isFinite else { return 0 }; return Int(max(Double(Int32.min),min(Double(Int32.max),value.rounded(.towardZero)))) }
        func text(_ value:Any?)->String {
            guard let value, !(value is NSNull) else { return "" }; return String(describing:value)
        }
        let patterns=(project["patterns"] as? [[String:Any]] ?? []).enumerated().map { index,item in
            ["id":integer(number(item,"id",Double(index))),"length":max(1,min(256,integer(number(item,"length",64))))]
        }
        let clips=(project["audioClips"] as? [[String:Any]] ?? []).map { item -> [String:Any] in
            ["id":text(item["id"]),"track":integer(number(item,"track",0)),"startBeat":number(item,"startBeat",0),
             "durationBeats":number(item,"durationBeats",0),"path":text(item["path"] ?? item["samplePath"]),
             "sourceOffset":max(0,min(1e6,number(item,"sourceOffset",number(item,"offsetSeconds",0)))),
             "muted":(item["muted"] ?? item["mute"]) as? Bool == true]
        }
        var result:[String:Any] = ["version":1,"linesPerBeat":max(1,min(32,integer(number(project,"linesPerBeat",4)))),
            "swing":Double(max(Float(0),min(Float(0.95),Float(number(project,"swing",0))))),
            "loopSong":project["loopSong"] as? Bool == true,
            // Tempo maps are retired; "timing" stays null for older readers of this projection.
            "bpm":max(20,min(999,number(project,"bpm",120))),"timing":NSNull(),
            "patterns":patterns,"sequence":(project["sequence"] as? [NSNumber] ?? []).map { integer($0.doubleValue) },"clips":clips]
        return result
    }
}
