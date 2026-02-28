import AppKit

let env = ProcessInfo.processInfo.environment
let alertsDir = env["CRANK_ALERTS_DIR"] ?? "\(NSHomeDirectory())/.crank/alerts"
let refreshInterval: TimeInterval = 5.0

func readCount() -> Int {
    return countAlertFiles(in: alertsDir)
}

func countAlertFiles(in dir: String) -> Int {
    guard let entries = try? FileManager.default.contentsOfDirectory(atPath: dir) else {
        return 0
    }
    return entries.filter { $0.hasSuffix(".json") }.count
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private var timer: Timer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.font = NSFont.monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        statusItem = item
        updateCount()

        timer = Timer.scheduledTimer(withTimeInterval: refreshInterval, repeats: true) { [weak self] _ in
            self?.updateCount()
        }

        let menu = NSMenu()
        menu.addItem(NSMenuItem(title: "Quit", action: #selector(quit), keyEquivalent: "q"))
        item.menu = menu
    }

    @objc private func quit() {
        NSApp.terminate(nil)
    }

    private func updateCount() {
        let count = readCount()
        let color: NSColor = count > 1 ? .systemRed : .labelColor
        let font = NSFont.monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        let attrs: [NSAttributedString.Key: Any] = [
            .foregroundColor: color,
            .font: font,
        ]
        statusItem?.button?.attributedTitle = NSAttributedString(
            string: String(count),
            attributes: attrs
        )
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.setActivationPolicy(.accessory)
app.delegate = delegate
app.run()
