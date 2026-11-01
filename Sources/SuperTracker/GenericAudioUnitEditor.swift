import AppKit
import AudioToolbox
import AVFoundation

/// Every AU remains keyboard editable even if its vendor did not supply an editor.
final class GenericAudioUnitEditor: NSViewController {
    private let unit: AUAudioUnit
    private var controls: [Int: (AUParameter, NSTextField)] = [:]
    init(unit: AUAudioUnit) { self.unit = unit; super.init(nibName: nil, bundle: nil) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func loadView() {
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 620, height: 480))
        scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        let stack = NSStackView()
        stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
        let parameters = unit.parameterTree?.allParameters ?? []
        if parameters.isEmpty { stack.addArrangedSubview(NSTextField(labelWithString: "This Audio Unit exposes no editable parameters.")) }
        for (index, parameter) in parameters.enumerated() {
            let row = NSStackView()
            row.orientation = .horizontal; row.spacing = 12
            let label = NSTextField(labelWithString: parameter.displayName)
            label.widthAnchor.constraint(equalToConstant: 190).isActive = true
            let slider = NSSlider(value: Double(parameter.value), minValue: Double(parameter.minValue),
                maxValue: Double(parameter.maxValue), target: self, action: #selector(changed(_:)))
            slider.tag = index; slider.isContinuous = true
            slider.isEnabled = parameter.flags.contains(.flag_IsWritable)
            slider.setAccessibilityLabel(parameter.displayName)
            slider.widthAnchor.constraint(equalToConstant: 220).isActive = true
            let value = NSTextField(labelWithString: AudioPluginParameterMetadata.display(parameter))
            value.widthAnchor.constraint(equalToConstant: 140).isActive = true
            row.addArrangedSubview(label); row.addArrangedSubview(slider); row.addArrangedSubview(value)
            stack.addArrangedSubview(row); controls[index] = (parameter, value)
        }
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = stack
        stack.widthAnchor.constraint(greaterThanOrEqualTo: scroll.contentView.widthAnchor).isActive = true
        view = scroll
    }
    @objc private func changed(_ sender: NSSlider) {
        guard let (parameter, label) = controls[sender.tag] else { return }
        parameter.setValue(sender.floatValue, originator: nil)
        label.stringValue = AudioPluginParameterMetadata.display(parameter)
    }
}
