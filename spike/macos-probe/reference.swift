import AppKit

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 350, y: 200, width: 800, height: 400),
                      styleMask: [.titled, .closable, .resizable],
                      backing: .buffered, defer: false)
window.title = "Cocoa IME control"
let text = NSTextView(frame: NSRect(x: 0, y: 0, width: 800, height: 400))
text.setAccessibilityIdentifier("editor-body")
text.string = "caf"
text.setSelectedRange(NSRange(location: 3, length: 0))
window.contentView = text
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(text)
app.run()
