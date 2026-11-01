import Foundation
import TrackerEngine

/// Physical parameter metadata of every built-in effect, for the shared effect page
/// descriptions in Resources/Web/effect-pages.js (bridge command `builtin.effects`).
/// The index of a parameter is its Audio Unit parameter address.
extension BuiltinEffectsAudioUnit {
    static func effectMetadata() -> [[String: Any]] {
        (0..<st_builtin_effect_count()).compactMap { index -> [String: Any]? in
            guard let info = st_builtin_effect_info(index)?.pointee else { return nil }
            let id = String(cString: info.id)
            let parameters: [[String: Any]] = (0..<info.parameterCount).compactMap { address in
                guard let p = st_builtin_effect_parameter_info(id, address)?.pointee else { return nil }
                let labels: [String] = (0..<p.labelCount).compactMap { label in
                    st_builtin_effect_parameter_label(id, address, label).map { String(cString: $0) }
                }
                return ["address": Int(address), "id": String(cString: p.id), "name": String(cString: p.name),
                        "unit": String(cString: p.unit), "min": p.minimum, "max": p.maximum, "defaultValue": p.defaultValue,
                        "logarithmic": p.flags & UInt32(ST_BUILTIN_PARAMETER_LOG) != 0,
                        "integer": p.flags & UInt32(ST_BUILTIN_PARAMETER_INTEGER) != 0,
                        "toggle": p.flags & UInt32(ST_BUILTIN_PARAMETER_TOGGLE) != 0,
                        "labels": labels]
            }
            return ["id": "stfx:" + id, "name": String(cString: info.name), "category": String(cString: info.category),
                    "sidechain": info.flags & UInt32(ST_BUILTIN_EFFECT_SIDECHAIN) != 0, "parameters": parameters]
        }
    }
}
