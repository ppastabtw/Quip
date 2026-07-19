import Cocoa
import InputMethodKit

func quipLog(_ message: String) {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let line = "\(formatter.string(from: Date())) pid=\(ProcessInfo.processInfo.processIdentifier) \(message)\n"
    let logs = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Logs", isDirectory: true)
    let file = logs.appendingPathComponent("QuipNativeIME.log")
    try? FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)
    if !FileManager.default.fileExists(atPath: file.path) {
        FileManager.default.createFile(atPath: file.path, contents: nil)
    }
    guard let handle = try? FileHandle(forWritingTo: file) else { return }
    defer { try? handle.close() }
    do {
        try handle.seekToEnd()
        try handle.write(contentsOf: Data(line.utf8))
    } catch {
        NSLog("Quip Native logging failed: %@", String(describing: error))
    }
}

class NSManualApplication: NSApplication {
    private let appDelegate = AppDelegate()

    override init() {
        super.init()
        delegate = appDelegate
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) is unavailable")
    }
}

@main
class AppDelegate: NSObject, NSApplicationDelegate {
    static var server = IMKServer()

    func applicationDidFinishLaunching(_ notification: Notification) {
        let connection = Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String
        AppDelegate.server = IMKServer(
            name: connection,
            bundleIdentifier: Bundle.main.bundleIdentifier
        )
        quipLog("process_started bundle=\(Bundle.main.bundleIdentifier ?? "unknown") connection=\(connection ?? "unknown")")
    }
}
