import AppKit

/// Presents interface-described context menus as real AppKit menus, so right-click
/// and Shift-F10 behave like a Mac app instead of a web page.
@MainActor
final class NativeContextMenu: NSObject {
    private var chosen: String?

    /// Items: {id, title, enabled?, checked?, separator?, key?, items?}. Returns the chosen id or nil.
    func show(_ items: [[String: Any]], at point: NSPoint, in view: NSView) -> String? {
        chosen = nil
        #if SELFTESTS
        if let pick = DevHarness.scriptedMenuPick {
            DevHarness.menuLog.append(["items": items])
            // Scripted runs pick by title (or "a/b" for submenu paths) without blocking on a menu.
            guard let pick else { return nil }
            var level = items
            for (index, title) in pick.split(separator: "/").enumerated() {
                guard let item = level.first(where: { $0["title"] as? String == String(title) }) else { return nil }
                if index == pick.split(separator: "/").count - 1 { return item["id"] as? String }
                level = item["items"] as? [[String: Any]] ?? []
            }
            return nil
        }
        #endif
        let menu = build(items)
        menu.autoenablesItems = false
        // WKWebView uses top-left coordinates, matching DOM client coordinates.
        let location = view.isFlipped ? point : NSPoint(x: point.x, y: view.bounds.height - point.y)
        menu.popUp(positioning: nil, at: location, in: view)
        return chosen
    }

    private func build(_ items: [[String: Any]]) -> NSMenu {
        let menu = NSMenu()
        menu.autoenablesItems = false
        for item in items.prefix(200) {
            if item["separator"] as? Bool == true { menu.addItem(.separator()); continue }
            let title = item["title"] as? String ?? ""
            if let header = item["header"] as? Bool, header {
                if #available(macOS 14.0, *) { menu.addItem(NSMenuItem.sectionHeader(title: title)) }
                else { let label = NSMenuItem(title: title, action: nil, keyEquivalent: ""); label.isEnabled = false; menu.addItem(label) }
                continue
            }
            let entry = NSMenuItem(title: title, action: #selector(choose(_:)), keyEquivalent: "")
            entry.target = self
            entry.representedObject = item["id"] as? String
            entry.isEnabled = item["enabled"] as? Bool ?? true
            entry.state = item["checked"] as? Bool == true ? .on : .off
            if let key = item["key"] as? String, !key.isEmpty { applyShortcut(key, to: entry) }
            if let children = item["items"] as? [[String: Any]], !children.isEmpty {
                entry.action = nil
                entry.submenu = build(children)
            }
            menu.addItem(entry)
        }
        return menu
    }

    /// Display-only shortcut hint such as "⌘C", "⌥A" or "⇧⌘Z".
    private func applyShortcut(_ text: String, to item: NSMenuItem) {
        var modifiers: NSEvent.ModifierFlags = []
        var rest = Substring(text)
        let symbols: [(Character, NSEvent.ModifierFlags)] = [("⌘", .command), ("⌥", .option), ("⇧", .shift), ("⌃", .control)]
        while let first = rest.first, let match = symbols.first(where: { $0.0 == first }) { modifiers.insert(match.1); rest = rest.dropFirst() }
        guard !rest.isEmpty else { return }
        let names: [String: String] = ["⌫": "\u{8}", "⌦": "\u{7f}", "↩": "\r", "Space": " ", "Tab": "\t", "Esc": "\u{1b}"]
        let key = names[String(rest)] ?? String(rest).lowercased()
        guard key.count == 1 else { return }
        item.keyEquivalent = key
        item.keyEquivalentModifierMask = modifiers
    }

    @objc private func choose(_ sender: NSMenuItem) { chosen = sender.representedObject as? String }
}
