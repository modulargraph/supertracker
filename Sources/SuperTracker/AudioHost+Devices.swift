import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Audio devices, the hardware input monitor path and audio configuration.
extension AudioHost {
    func audioDevices() -> [String:Any] {
        var address = AudioObjectPropertyAddress(mSelector:kAudioHardwarePropertyDevices,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var size:UInt32 = 0
        AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject),&address,0,nil,&size)
        var ids = [AudioDeviceID](repeating:0,count:Int(size)/MemoryLayout<AudioDeviceID>.size)
        AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject),&address,0,nil,&size,&ids)
        func name(_ device:AudioDeviceID)->String {
            var property = AudioObjectPropertyAddress(mSelector:kAudioObjectPropertyName,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
            var value:Unmanaged<CFString>?; var bytes = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
            guard AudioObjectGetPropertyData(device,&property,0,nil,&bytes,&value) == noErr, let value else { return "Audio Device" }
            return value.takeUnretainedValue() as String
        }
        func uid(_ device:AudioDeviceID)->String {
            var property = AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyDeviceUID,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
            var value:Unmanaged<CFString>?; var bytes = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
            guard AudioObjectGetPropertyData(device,&property,0,nil,&bytes,&value) == noErr, let value else { return "" }
            return value.takeUnretainedValue() as String
        }
        func channels(_ device:AudioDeviceID,_ scope:AudioObjectPropertyScope)->Int {
            var property = AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyStreamConfiguration,mScope:scope,mElement:kAudioObjectPropertyElementMain)
            var bytes:UInt32 = 0
            guard AudioObjectGetPropertyDataSize(device,&property,0,nil,&bytes) == noErr, bytes > 0 else { return 0 }
            let pointer = UnsafeMutableRawPointer.allocate(byteCount:Int(bytes),alignment:MemoryLayout<AudioBufferList>.alignment)
            defer { pointer.deallocate() }
            guard AudioObjectGetPropertyData(device,&property,0,nil,&bytes,pointer) == noErr else { return 0 }
            return UnsafeMutableAudioBufferListPointer(pointer.assumingMemoryBound(to:AudioBufferList.self)).reduce(0) { $0 + Int($1.mNumberChannels) }
        }
        func defaultID(_ selector:AudioObjectPropertySelector)->UInt32 {
            var property = AudioObjectPropertyAddress(mSelector:selector,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
            var value:UInt32 = 0; var bytes = UInt32(MemoryLayout<UInt32>.size)
            AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject),&property,0,nil,&bytes,&value); return value
        }
        let devices:[[String:Any]] = ids.map { id in
            var property = AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyNominalSampleRate,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
            var rate:Double = 0; var bytes = UInt32(MemoryLayout<Double>.size)
            AudioObjectGetPropertyData(id,&property,0,nil,&bytes,&rate)
            var frames:UInt32 = 0; bytes = UInt32(MemoryLayout<UInt32>.size); property.mSelector = kAudioDevicePropertyBufferFrameSize
            AudioObjectGetPropertyData(id,&property,0,nil,&bytes,&frames)
            return ["id":id,"uid":uid(id),"name":name(id),"inputChannels":channels(id,kAudioObjectPropertyScopeInput),"outputChannels":channels(id,kAudioObjectPropertyScopeOutput),"sampleRate":rate,"bufferSize":frames]
        }
        let activeOutput = selectedOutputDevice ?? defaultID(kAudioHardwarePropertyDefaultOutputDevice)
        var engineDevice:AudioDeviceID=0
        if let unit=graph.outputNode.audioUnit {
            var bytes=UInt32(MemoryLayout<AudioDeviceID>.size)
            _ = AudioUnitGetProperty(unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&engineDevice,&bytes)
        }
        let bufferSize = devices.first { ($0["id"] as? UInt32) == activeOutput }?["bufferSize"] ?? 256
        return ["inputAcknowledgements":AudioInputCapture.capability,"devices":devices,"inputs":devices.filter { ($0["inputChannels"] as? Int ?? 0)>0 },"outputs":devices.filter { ($0["outputChannels"] as? Int ?? 0)>0 },
            "defaultInput":defaultID(kAudioHardwarePropertyDefaultInputDevice),"defaultOutput":defaultID(kAudioHardwarePropertyDefaultOutputDevice),"sampleRate":sampleRate,
            "engineDevice":engineDevice,"outputDevice":activeOutput,"inputDevice":selectedInputDevice ?? defaultID(kAudioHardwarePropertyDefaultInputDevice),"bufferSize":bufferSize,
            "lineInputs":lineInputStatus(),"lineInputError":lineInputError as Any? ?? NSNull(),"bufferSizes":[64,128,256,512,1024,2048],"inputChannels":selectedInputChannels,"inputMetering":inputTapInstalled,"monitorVolume":inputMixer?.outputVolume ?? 0,"monitoring":(inputMixer?.outputVolume ?? 0) > 0,"running":graph.isRunning,"latencyCompensation":rack.latencyStatus()]
    }

    @MainActor func configureAudio(_ payload:[String:Any]) async throws -> [String:Any] {
        guard recording == nil else {
            throw AudioSampleError.invalid("Finish recording before reconfiguring the audio graph or device.")
        }
        rack.notifyStateCaptureMutation()
        defer { rack.notifyStateCaptureMutation() }
        guard audioConfigurationsInFlight == 0 else { throw AudioSampleError.invalid("Wait for the current audio configuration to finish.") }
        audioConfigurationsInFlight += 1
        defer { audioConfigurationsInFlight -= 1 }
        let wasRunning = graph.isRunning
        let previousMonitor = inputMixer?.outputVolume
        let previousPair=AudioDeviceSelection.Pair(input:selectedInputDevice ?? AudioDeviceSelection.defaultDevice(input:true),output:selectedOutputDevice ?? AudioDeviceSelection.defaultDevice(input:false))
        let previousChannels=selectedInputChannels
        var selectionUsesInput=false
        graph.stop(); renderCacheNeedsReset = true
        do {
            let requestedInput=(payload["inputDevice"] as? NSNumber ?? payload["input"] as? NSNumber)?.uint32Value
            let requestedOutput=(payload["outputDevice"] as? NSNumber ?? payload["output"] as? NSNumber)?.uint32Value
            if requestedInput != nil || requestedOutput != nil {
                if requestedInput != nil { try await authorizeInput();try requireInteractiveMutation();try requireNoActiveRecording() }
                let current=AudioDeviceSelection.Pair(input:selectedInputDevice ?? AudioDeviceSelection.defaultDevice(input:true),
                    output:selectedOutputDevice ?? AudioDeviceSelection.defaultDevice(input:false))
                let next=AudioDeviceSelection.Pair(input:requestedInput ?? current.input,output:requestedOutput ?? current.output)
                guard let outputUnit=graph.outputNode.audioUnit else {
                    throw AudioSampleError.invalid("The audio input/output unit is unavailable. Reconnect the device and try again.")
                }
                // Touching graph.inputNode instantiates the HAL input (mic indicator,
                // permission prompt, Bluetooth call mode, an aggregate with the
                // default microphone). Output-only changes select the output alone.
                selectionUsesInput=requestedInput != nil || hardwareInputActive || payload["inputChannels"] != nil ||
                    payload["monitor"] as? Bool == true || payload["inputMeter"] as? Bool == true
                let inputUnit=selectionUsesInput ? graph.inputNode.audioUnit ?? outputUnit : outputUnit
                if let channels=payload["inputChannels"] as? [Int] {
                    let count=AudioDeviceSelection.channels(next.input,input:true)
                    guard !channels.isEmpty,channels.count <= 2,channels.allSatisfy({$0 >= 0 && $0 < count}) else {
                        throw AudioSampleError.invalid("Choose one or two available channels for the selected microphone or interface.")
                    }
                }
                let changesInput=selectionUsesInput && next.input != current.input
                if changesInput {
                    if inputTapInstalled { graph.inputNode.removeTap(onBus:0);inputTapInstalled=false;meters.resetInput() }
                    if let mixer=inputMixer {
                        graph.disconnectNodeOutput(graph.inputNode);graph.disconnectNodeOutput(mixer);graph.detach(mixer);inputMixer=nil
                        removeInputMonitorSelector()
                    }
                }
                do {
                    try deviceSelection.select(selectionUsesInput ? next : .init(input:0,output:next.output),
                        current:selectionUsesInput ? current : .init(input:0,output:current.output),inputUnit:inputUnit,outputUnit:outputUnit)
                }
                catch {
                    if changesInput,let previousMonitor { try? connectInput();inputMixer?.outputVolume=previousMonitor }
                    throw error
                }
                selectedInputDevice=next.input;selectedOutputDevice=next.output
                if selectionUsesInput && next.input != current.input { recheckLineInputsSoon() }
                if changesInput,let previousMonitor { try connectInput();inputMixer?.outputVolume=previousMonitor }
            }
            if let frameValue = payload["bufferSize"] as? NSNumber, let unit = graph.outputNode.audioUnit {
                var device:AudioDeviceID = 0; var bytes = UInt32(MemoryLayout<AudioDeviceID>.size)
                AudioUnitGetProperty(unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&device,&bytes)
                var property = AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyBufferFrameSize,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
                var frames = UInt32(max(32,min(4096,frameValue.intValue)))
                let result = AudioObjectSetPropertyData(device,&property,0,nil,UInt32(MemoryLayout<UInt32>.size),&frames)
                guard result == noErr else { throw AudioSampleError.invalid("This audio device does not support the requested buffer size.") }
            }
            if let selected = payload["inputChannels"] as? [Int] {
                try await authorizeInput()
                try requireInteractiveMutation(); try requireNoActiveRecording()
                let count = Int(engineInputFormat.channelCount)
                guard !selected.isEmpty, selected.count <= 2, selected.allSatisfy({ $0 >= 0 && $0 < count }) else { throw AudioSampleError.invalid("Choose one or two valid audio input channels, numbered from zero.") }
                selectedInputChannels = selected
                if inputMixer != nil { try updateInputMonitorSelector() }
                if inputTapInstalled { installInputTap() }
            }
            if let monitor = payload["monitor"] as? Bool {
                if monitor { try await authorizeInput(); try requireInteractiveMutation(); try requireNoActiveRecording(); try connectInput() }
                inputMixer?.outputVolume = monitor && lineInputs.isEmpty ? Float(PayloadValue.number(payload,"monitorVolume",0.5)) : 0
            }
            if let armed = payload["inputMeter"] as? Bool {
                if armed { try await authorizeInput(); try requireInteractiveMutation(); try requireNoActiveRecording(); try connectInput() }
                else if (recording == nil || recording?.source == .master), (inputMixer?.outputVolume ?? 0) == 0 {
                    if inputTapInstalled { graph.inputNode.removeTap(onBus:0); inputTapInstalled = false }
                    if let mixer = inputMixer {
                        graph.disconnectNodeOutput(graph.inputNode); graph.disconnectNodeOutput(mixer); graph.detach(mixer); inputMixer = nil
                        removeInputMonitorSelector()
                    }
                    meters.resetInput()
                    if !lineInputs.isEmpty { wireLineInputFanout() }
                }
            }
            if !lineInputs.isEmpty || !lineInputPaused.isEmpty || projectHasLineInputs { try synchronizeLineInputs(project,force:true) }
            // Meter arming may have just created and connected the input mixer.
            // Apply the requested monitor gain after every connection is complete.
            if let monitor = payload["monitor"] as? Bool {
                inputMixer?.outputVolume = monitor && lineInputs.isEmpty ? Float(PayloadValue.number(payload,"monitorVolume",0.5)) : 0
            }
            // A new buffer size or input device changes how late hardware returns arrive.
            refreshHardwareReturnLatency()
            if wasRunning || payload["monitor"] as? Bool == true || payload["inputMeter"] as? Bool == true { try start() }
            return audioDevices()
        } catch {
            let original=error
            if let outputUnit=graph.outputNode.audioUnit {
                let current=AudioDeviceSelection.Pair(input:selectedInputDevice ?? previousPair.input,output:selectedOutputDevice ?? previousPair.output)
                if current != previousPair {
                    do {
                        if selectionUsesInput || hardwareInputActive {
                            try deviceSelection.select(previousPair,current:current,inputUnit:graph.inputNode.audioUnit ?? outputUnit,outputUnit:outputUnit)
                        } else {
                            try deviceSelection.select(.init(input:0,output:previousPair.output),current:.init(input:0,output:current.output),inputUnit:outputUnit,outputUnit:outputUnit)
                        }
                        selectedInputDevice=previousPair.input;selectedOutputDevice=previousPair.output;selectedInputChannels=previousChannels
                        if inputTapInstalled { installInputTap() }
                    } catch { emit("audio.error",["error":"The previous input/output pair could not be restored: "+error.localizedDescription]) }
                }
            }
            if wasRunning { try? start() }
            throw original
        }
    }

    /// True when the graph already owns a hardware input path.
    var hardwareInputActive:Bool { inputMixer != nil || inputTapInstalled || !lineInputs.isEmpty }

    @MainActor func authorizeInput() async throws {
        switch AVCaptureDevice.authorizationStatus(for:.audio) {
        case .authorized: return
        case .notDetermined:
            let granted = await AVCaptureDevice.requestAccess(for:.audio)
            if granted { return }
        default: break
        }
        throw AudioSampleError.invalid("Microphone access is required for recording and hardware synth returns. Enable SuperTracker in System Settings → Privacy & Security → Microphone.")
    }

    func connectInput() throws {
        guard inputMixer == nil else { return }
        let input = graph.inputNode
        let format = engineInputFormat
        guard format.channelCount > 0, format.sampleRate > 0 else { throw AudioSampleError.invalid("No audio input is available. Connect an interface and choose its input device.") }
        let mixer = AVAudioMixerNode(); mixer.outputVolume = 0
        graph.attach(mixer)
        mixer.outputVolume = 0
        graph.connect(mixer,to:graph.mainMixerNode,format:AVAudioFormat(standardFormatWithSampleRate:format.sampleRate,channels:2))
        inputMixer = mixer
        try updateInputMonitorSelector()
        // Connecting a mixer initializes its AU parameters; mute only after wiring.
        mixer.outputVolume = 0
        installInputTap()
    }

    /// A shared unmapped tap meters and records original hardware channels.
    /// Never set channelMap on the duplex HAL unit: it also owns output. The tap
    /// thread reads an immutable channel copy; reinstall after channels change.
    func installInputTap() {
        let input = graph.inputNode, format = engineInputFormat, channels = selectedInputChannels
        if inputTapInstalled { input.removeTap(onBus:0); inputTapInstalled = false }
        input.installTap(onBus:0,bufferSize:2048,format:format) { [weak self] buffer,time in
            guard let self else { return }
            self.meters.measureInput(buffer,channels:channels.filter { $0 < Int(buffer.format.channelCount) })
            let value = OSAtomicAdd64Barrier(0,self.inputRecordingEndpoint)
            if value != 0, let pointer = UnsafeRawPointer(bitPattern:UInt(UInt64(bitPattern:value))) {
                Unmanaged<AudioRecording>.fromOpaque(pointer).takeUnretainedValue().receive(buffer,at:time)
            }
        }
        inputTapInstalled = true
    }

    /// The engine's input stream format. After a device change AVAudioEngine keeps reporting the previous
    /// device's format on the input node's output side (for example 1 channel from a built-in microphone
    /// after switching to a 10-input interface) until the node is reconnected; its hardware side is current.
    /// Connecting at this format brings the output side up to date.
    var engineInputFormat: AVAudioFormat {
        let input = graph.inputNode, hardware = input.inputFormat(forBus:0), stream = input.outputFormat(forBus:0)
        guard hardware.channelCount > 0, hardware.sampleRate > 0 else { return stream }
        return hardware.channelCount != stream.channelCount || hardware.sampleRate != stream.sampleRate ? hardware : stream
    }

    /// After a device change the hardware input may have a new rate or channel
    /// count. Reconnect the monitor mixer, selector and tap at the current format.
    func rebuildInputMonitorPath() throws {
        guard let mixer = inputMixer else { return }
        let format = engineInputFormat
        guard format.channelCount > 0, format.sampleRate > 0 else { throw AudioSampleError.invalid("No audio input is available. Connect an interface and choose its input device.") }
        let gain = mixer.outputVolume
        graph.connect(mixer,to:graph.mainMixerNode,format:AVAudioFormat(standardFormatWithSampleRate:format.sampleRate,channels:2))
        try updateInputMonitorSelector()
        mixer.outputVolume = gain
        if inputTapInstalled { installInputTap() }
    }

    func removeInputMonitorPath() {
        if inputTapInstalled { graph.inputNode.removeTap(onBus:0); inputTapInstalled = false }
        if let mixer = inputMixer {
            graph.disconnectNodeOutput(graph.inputNode); graph.disconnectNodeOutput(mixer); graph.detach(mixer); inputMixer = nil
            removeInputMonitorSelector()
        }
        meters.resetInput()
    }
}
