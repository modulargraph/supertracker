import AVFoundation
import AudioToolbox

/// A control-thread-built, immutable routing publication. The rack lends it to
/// audio callbacks through AudioEndpointStore; graph changes retire it only after
/// the old graph and every registered reader have finished.
final class AudioRoutingRender {
    struct MixEndpoint {
        let control: AudioMixAutomation
        let offset: Int64
    }
    struct PluginEndpoint {
        let parameter: AUScheduleParameterBlock
        let ranges: [UInt64:(Float,Float)]
        let offset: Int64
        let clock: AudioPluginRack.RenderClockOffset?
        init(unit:AUAudioUnit,offset:Int,clock:AudioPluginRack.RenderClockOffset? = nil) {
            parameter = unit.scheduleParameterBlock; self.clock = clock
            ranges = Dictionary((unit.parameterTree?.allParameters ?? []).map { ($0.address,($0.minValue,$0.maxValue)) },uniquingKeysWith:{ first,_ in first })
            self.offset = Int64(offset)
        }
    }
    enum Binding {
        case mix(MixEndpoint)
        case send(MixEndpoint)
        case plugin(PluginEndpoint)
        case unavailable
    }
    let standard:Bool
    /// AudioRoutingAutomation.hostMap of the mapping these bindings follow (0 = any).
    let hostMap:UInt64
    let bindings:[Binding]
    let tracks:[MixEndpoint?]
    let delaySends:[MixEndpoint?]
    let reverbSends:[MixEndpoint?]
    let pluginOffsets:[Int64]
    private let controls:[AudioMixAutomation]

    /// Plug-in destination with its parameter ranges in sorted raw storage.
    private final class PluginTarget {
        let parameter: AUScheduleParameterBlock
        let offset: Int64
        let clock: AudioPluginRack.RenderClockOffset?
        let count: Int
        let addresses: UnsafeMutablePointer<UInt64>
        let ranges: UnsafeMutablePointer<(Float,Float)>
        init(_ endpoint:PluginEndpoint) {
            parameter = endpoint.parameter; offset = endpoint.offset; clock = endpoint.clock
            let sorted = endpoint.ranges.sorted { $0.key < $1.key }
            count = sorted.count
            addresses = .allocate(capacity:max(1,count)); ranges = .allocate(capacity:max(1,count))
            for (index,item) in sorted.enumerated() { (addresses+index).initialize(to:item.key); (ranges+index).initialize(to:item.value) }
        }
        deinit { addresses.deinitialize(count:count); addresses.deallocate(); ranges.deinitialize(count:count); ranges.deallocate() }
        func automate(address:UInt64,value:Double,sampleTime:Int64) {
            var low = 0, high = count
            while low < high { let middle = (low+high)/2; if addresses[middle] < address { low = middle+1 } else { high = middle } }
            guard low < count, addresses[low] == address else { return }
            let range = ranges[low]
            parameter(sampleTime+offset+(clock.map { atomicLoad($0.frames) } ?? 0),0,address,range.0+Float(max(0,min(1,value)))*(range.1-range.0))
        }
    }
    /// Render-thread lookup tables: plain kinds/offsets and unretained references whose
    /// owners (`bindings`, `tracks`, sends and `pluginTargets`) live as long as this
    /// publication, so automation delivery performs no retain/release or payload copy.
    private struct MixSlot { var control:Unmanaged<AudioMixAutomation>?; var offset:Int64 }
    private let pluginTargets:[PluginTarget]
    private let kinds:UnsafeMutablePointer<UInt8>          // 0 unavailable, 1 mix, 2 send, 3 plug-in
    private let mixSlots:UnsafeMutablePointer<MixSlot>     // per binding (mix/send)
    private let pluginSlots:UnsafeMutablePointer<Unmanaged<PluginTarget>?>
    private let trackSlots:UnsafeMutablePointer<MixSlot>   // tracks, delay sends, reverb sends
    private let bindingCount:Int,trackCount:Int,delayCount:Int,reverbCount:Int

    init(standard:Bool,hostMap:UInt64 = 0,bindings:[Binding],tracks:[MixEndpoint?],delaySends:[MixEndpoint?],
         reverbSends:[MixEndpoint?],pluginOffsets:[Int64],controls:[AudioMixAutomation]) {
        self.standard = standard; self.hostMap = hostMap; self.bindings = bindings; self.tracks = tracks
        self.delaySends = delaySends; self.reverbSends = reverbSends; self.pluginOffsets = pluginOffsets
        var unique = Set<ObjectIdentifier>()
        self.controls = controls.filter { unique.insert(ObjectIdentifier($0)).inserted }
        func slot(_ endpoint:MixEndpoint?)->MixSlot { MixSlot(control:endpoint.map { Unmanaged.passUnretained($0.control) },offset:endpoint?.offset ?? 0) }
        bindingCount = bindings.count; trackCount = tracks.count; delayCount = delaySends.count; reverbCount = reverbSends.count
        kinds = .allocate(capacity:max(1,bindingCount)); mixSlots = .allocate(capacity:max(1,bindingCount))
        pluginSlots = .allocate(capacity:max(1,bindingCount))
        trackSlots = .allocate(capacity:max(1,trackCount+delayCount+reverbCount))
        var targets:[PluginTarget] = []
        for (index,binding) in bindings.enumerated() {
            var kind:UInt8 = 0, mix = MixSlot(control:nil,offset:0), plugin:Unmanaged<PluginTarget>? = nil
            switch binding {
            case .mix(let endpoint): kind = 1; mix = slot(endpoint)
            case .send(let endpoint): kind = 2; mix = slot(endpoint)
            case .plugin(let endpoint): kind = 3; let target = PluginTarget(endpoint); targets.append(target); plugin = .passUnretained(target)
            case .unavailable: break
            }
            (kinds+index).initialize(to:kind); (mixSlots+index).initialize(to:mix); (pluginSlots+index).initialize(to:plugin)
        }
        pluginTargets = targets
        for (index,endpoint) in (tracks+delaySends+reverbSends).enumerated() { (trackSlots+index).initialize(to:slot(endpoint)) }
    }
    deinit {
        kinds.deinitialize(count:bindingCount); kinds.deallocate()
        mixSlots.deinitialize(count:bindingCount); mixSlots.deallocate()
        pluginSlots.deinitialize(count:bindingCount); pluginSlots.deallocate()
        trackSlots.deinitialize(count:trackCount+delayCount+reverbCount); trackSlots.deallocate()
    }

    /// Raw mix/send values and normalized plug-in values are converted only at
    /// their final endpoint. Destination lookup is a bounded array access.
    /// A routed table (hostMap != 0) drops an event numbered for another destination
    /// map (`map`, from the engine project that emitted it): its index may now name
    /// another endpoint. The engine re-emits its lanes once project and table agree.
    func automate(destination:Int,address:UInt64,value:Double,sampleTime:Int64,map:UInt64 = 0) {
        guard destination >= 0, destination < bindingCount, value.isFinite else { return }
        guard hostMap == 0 || map == hostMap else { return }
        switch kinds[destination] {
        case 1,2:
            let mix = mixSlots[destination]
            guard let control = mix.control, kinds[destination] == 1 ? address == 0 || address == 1 : address == 0 else { return }
            let target = kinds[destination] == 1 ? Int(address) : 4
            control._withUnsafeGuaranteedRef { $0.schedule(target:target,value:Float(value),sampleTime:sampleTime+mix.offset) }
        case 3:
            pluginSlots[destination]?._withUnsafeGuaranteedRef { $0.automate(address:address,value:value,sampleTime:sampleTime) }
        default: break
        }
    }

    /// Standard native/AU strips share tracker volume/pan and both send aliases.
    /// Instrument-level controls continue through their original source processor.
    @discardableResult func automateTrack(track:Int,target:Int,value:Float,sampleTime:Int64) -> Bool {
        guard standard, track >= 0, track < trackCount else { return false }
        let index:Int
        switch target {
        case 0,1: index = track
        case 4: guard track < delayCount else { return true }; index = trackCount+track
        case 5: guard track < reverbCount else { return true }; index = trackCount+delayCount+track
        default: return false
        }
        let mix = trackSlots[index]
        mix.control?._withUnsafeGuaranteedRef { $0.schedule(target:target,value:value,sampleTime:sampleTime+mix.offset) }
        return true
    }

    func resetAutomation() { for control in controls { control.reset() } }
    var droppedEvents:Int { controls.reduce(0) { $0+$1.droppedEvents } }
}
