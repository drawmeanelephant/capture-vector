import AppKit
import ApplicationServices
import Carbon

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

func point(_ element: AXUIElement, _ name: String) -> [CGFloat]? {
    guard let raw = attribute(element, name), CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    let value = unsafeBitCast(raw, to: AXValue.self)
    if name == kAXPositionAttribute {
        var point = CGPoint.zero
        if AXValueGetValue(value, .cgPoint, &point) { return [point.x, point.y] }
    } else {
        var size = CGSize.zero
        if AXValueGetValue(value, .cgSize, &size) { return [size.width, size.height] }
    }
    return nil
}

func describe(_ element: AXUIElement, depth: Int = 0) -> [String: Any] {
    var result: [String: Any] = [:]
    for name in [kAXRoleAttribute, kAXTitleAttribute, kAXDescriptionAttribute,
                 kAXIdentifierAttribute, kAXValueAttribute, kAXFocusedAttribute] {
        if let value = attribute(element, name) {
            if let string = value as? String { result[name] = string }
            else if let number = value as? NSNumber { result[name] = number }
        }
    }
    result["position"] = point(element, kAXPositionAttribute)
    result["size"] = point(element, kAXSizeAttribute)
    if depth < 16, let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] {
        result["children"] = children.map { describe($0, depth: depth + 1) }
    }
    return result
}

func sourceID(_ source: TISInputSource) -> String {
    guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return "" }
    return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
}

let args = Array(CommandLine.arguments.dropFirst())
guard let command = args.first else { fail("Expected inspect/windows/focus/press/replace/key/close/quit/source") }
if command == "source" {
    let previous = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
    if args.count == 1 { print(sourceID(previous)); exit(0) }
    let sources = TISCreateInputSourceList(nil, true).takeRetainedValue() as! [TISInputSource]
    guard args.count == 2 else { fail("Expected source ID") }
    let id = args[1]
    guard let source = sources.first(where: { sourceID($0) == id }) else {
        fail("Input source is not installed: \(id)")
    }
    guard TISSelectInputSource(source) == noErr else {
        fail("Input source is not enabled: \(args[1]). Enable it in Keyboard > Input Sources first.")
    }
    print(sourceID(previous))
    exit(0)
}
guard AXIsProcessTrusted() else { fail("Accessibility access is required for native probes.") }
guard args.count > 1, let pid = Int32(args[1]) else { fail("Expected app PID") }
guard let app = NSRunningApplication(processIdentifier: pid) else {
    if command == "inspect" { print("[]"); exit(0) }
    fail("Expected live app PID")
}
let element = AXUIElementCreateApplication(pid)
if command == "quit" {
    guard app.terminate() else { fail("Native quit request failed") }
    exit(0)
}
let windows = attribute(element, kAXWindowsAttribute) as? [AXUIElement] ?? []
if command == "inspect" {
    let data = try JSONSerialization.data(withJSONObject: windows.map { describe($0) }, options: [.sortedKeys])
    print(String(decoding: data, as: UTF8.self))
    exit(0)
}
if command == "windows" {
    let info = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements],
                                        kCGNullWindowID) as! [[String: Any]]
    let own = info.filter { $0[kCGWindowOwnerPID as String] as? Int32 == pid }
    let data = try JSONSerialization.data(withJSONObject: own, options: [.sortedKeys])
    print(String(decoding: data, as: UTF8.self))
    exit(0)
}
if command == "focus" || command == "press" || command == "replace" {
    guard args.count == (command == "replace" ? 4 : 3) else { fail("Expected element identifier") }
    func find(_ root: AXUIElement) -> AXUIElement? {
        if attribute(root, kAXIdentifierAttribute) as? String == args[2] { return root }
        for child in attribute(root, kAXChildrenAttribute) as? [AXUIElement] ?? [] {
            if let found = find(child) { return found }
        }
        return nil
    }
    for window in windows {
        if let field = find(window) {
            if command == "replace" {
                guard AXUIElementSetAttributeValue(field, kAXValueAttribute as CFString,
                                                   args[3] as CFString) == .success else {
                    fail("Replace failed for \(args[2])")
                }
                exit(0)
            }
            if command == "press" {
                guard AXUIElementPerformAction(field, kAXPressAction as CFString) == .success else {
                    fail("Press failed for \(args[2])")
                }
                exit(0)
            }
            guard AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString,
                                               kCFBooleanTrue) == .success,
                  AXUIElementPerformAction(window, kAXRaiseAction as CFString) == .success,
                  AXUIElementSetAttributeValue(field, kAXFocusedAttribute as CFString,
                                               kCFBooleanTrue) == .success else { fail("Focus failed") }
            exit(0)
        }
    }
    fail("No element \(args[2])")
}
if command == "key" {
    for _ in 0..<20 {
        if app.isActive { break }
        Thread.sleep(forTimeInterval: 0.05)
    }
    guard app.isActive else {
        let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "unknown"
        fail("Target app must be frontmost before sending native keys (frontmost: \(front)).")
    }
    guard args.count > 2, let code = UInt16(args[2]) else { fail("Expected virtual key code") }
    var flags: CGEventFlags = []
    for flag in args.dropFirst(3) {
        switch flag {
        case "cmd": flags.insert(.maskCommand)
        case "alt": flags.insert(.maskAlternate)
        case "shift": flags.insert(.maskShift)
        default: fail("Unknown modifier: \(flag)")
        }
    }
    let source = CGEventSource(stateID: .privateState)
    func post(_ code: UInt16, _ down: Bool, _ flags: CGEventFlags, modifier: Bool = false) {
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else {
            fail("Could not create keyboard event")
        }
        if modifier { event.type = .flagsChanged }
        event.flags = flags
        event.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.05)
    }
    let modifiers: [(CGEventFlags, UInt16)] =
        [(.maskCommand, 55), (.maskAlternate, 58), (.maskShift, 56)].filter { flags.contains($0.0) }
    for (_, key) in modifiers { post(key, true, flags, modifier: true) }
    for down in [true, false] { post(code, down, flags) }
    for (_, key) in modifiers { post(key, false, [], modifier: true) }
} else if command == "close" {
    guard args.count == 3, let index = Int(args[2]), windows.indices.contains(index),
          let button = attribute(windows[index], kAXCloseButtonAttribute) else {
        fail("Expected window index with a close button")
    }
    guard AXUIElementPerformAction(unsafeBitCast(button, to: AXUIElement.self),
                                  kAXPressAction as CFString) == .success else { fail("Close failed") }
} else {
    fail("Unknown command: \(command)")
}
