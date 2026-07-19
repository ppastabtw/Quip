import AppKit
import Foundation
import Network

extension Notification.Name {
    static let quipEngineBridgeMessage = Notification.Name("QuipEngineBridgeMessage")
}

final class QuipEngineBridge {
    static let shared = QuipEngineBridge()

    private let queue = DispatchQueue(label: "com.hackthe6ix.quip.native-engine-bridge")
    private let port = NWEndpoint.Port(rawValue: 48731)!
    private var connection: NWConnection?
    private var receiveBuffer = Data()
    private var pendingMessages: [Data] = []
    private var awaitingCaptureAcknowledgments: [
        String: (generation: UInt, data: Data)
    ] = [:]
    private var ready = false
    private var reconnectScheduled = false
    private var attemptedAppLaunch = false

    private init() {}

    func connect() {
        queue.async { [weak self] in
            self?.connectOnQueue()
        }
    }

    func capture(
        sessionID: String,
        generation: UInt,
        draft: String,
        caret: NSRect
    ) {
        let primaryTop = NSScreen.screens.first.map { $0.frame.maxY } ?? 0
        send([
            "type": "capture",
            "session_id": sessionID,
            "generation": generation,
            "draft": draft,
            "caret": [
                "x": caret.origin.x,
                "y": primaryTop - caret.maxY,
                "width": max(caret.width, 1),
                "height": caret.height,
            ],
        ])
    }

    func select(sessionID: String, destinationID: String, index: Int) {
        send([
            "type": "select",
            "session_id": sessionID,
            "destination_id": destinationID,
            "index": index,
        ])
    }

    func accept(sessionID: String, destinationID: String) {
        send([
            "type": "accept",
            "session_id": sessionID,
            "destination_id": destinationID,
        ])
    }

    func move(sessionID: String, destinationID: String, delta: Int) {
        send([
            "type": "move",
            "session_id": sessionID,
            "destination_id": destinationID,
            "delta": delta,
        ])
    }

    func dismiss(sessionID: String) {
        send([
            "type": "dismiss",
            "session_id": sessionID,
        ])
    }

    private func send(_ object: [String: Any]) {
        guard JSONSerialization.isValidJSONObject(object),
              var data = try? JSONSerialization.data(withJSONObject: object)
        else {
            quipLog("engine_bridge_encode_failed")
            return
        }
        data.append(0x0A)
        queue.async { [weak self] in
            guard let self else { return }
            if let (sessionID, generation) = self.captureIdentity(from: data) {
                self.awaitingCaptureAcknowledgments[sessionID] = (
                    generation: generation,
                    data: data
                )
                self.scheduleCaptureAcknowledgmentTimeout(
                    sessionID: sessionID,
                    generation: generation
                )
            }
            if self.ready, let connection = self.connection {
                self.sendOnQueue(data, through: connection)
            } else {
                self.enqueuePending(data)
                self.connectOnQueue()
            }
        }
    }

    private func captureIdentity(from data: Data) -> (sessionID: String, generation: UInt)? {
        guard let decoded = try? JSONSerialization.jsonObject(
            with: data.dropLast()
        ) as? [String: Any],
              decoded["type"] as? String == "capture",
              let sessionID = decoded["session_id"] as? String,
              let generation = (decoded["generation"] as? NSNumber)?.uintValue
        else { return nil }
        return (sessionID, generation)
    }

    private func enqueuePending(_ data: Data) {
        if let (sessionID, _) = captureIdentity(from: data) {
            pendingMessages.removeAll { pending in
                captureIdentity(from: pending)?.sessionID == sessionID
            }
        }
        pendingMessages.append(data)
    }

    private func scheduleCaptureAcknowledgmentTimeout(
        sessionID: String,
        generation: UInt
    ) {
        queue.asyncAfter(deadline: .now() + 1) { [weak self] in
            guard let self,
                  let pending = self.awaitingCaptureAcknowledgments[sessionID],
                  pending.generation == generation
            else { return }
            quipLog(
                "engine_bridge_capture_ack_timeout session=\(sessionID) generation=\(generation)"
            )
            self.enqueuePending(pending.data)
            if let connection = self.connection {
                self.disconnectOnQueue(connection)
            }
            self.connectOnQueue()
            self.scheduleCaptureAcknowledgmentTimeout(
                sessionID: sessionID,
                generation: generation
            )
        }
    }

    private func connectOnQueue() {
        guard connection == nil else { return }
        let connection = NWConnection(host: "127.0.0.1", port: port, using: .tcp)
        self.connection = connection
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard let self, let connection else { return }
            self.queue.async {
                guard self.connection === connection else { return }
                switch state {
                case .ready:
                    self.ready = true
                    self.reconnectScheduled = false
                    quipLog("engine_bridge_connected port=48731")
                    let pending = self.pendingMessages
                    self.pendingMessages.removeAll(keepingCapacity: true)
                    for message in pending {
                        self.sendOnQueue(message, through: connection)
                    }
                    self.receiveNext(on: connection)
                case .failed(let error):
                    quipLog("engine_bridge_failed error=\(error)")
                    self.disconnectOnQueue(connection)
                    self.launchQuipAppIfNeeded()
                    self.scheduleReconnect()
                case .cancelled:
                    self.disconnectOnQueue(connection)
                    self.scheduleReconnect()
                default:
                    break
                }
            }
        }
        connection.start(queue: queue)
    }

    private func sendOnQueue(_ data: Data, through connection: NWConnection) {
        connection.send(content: data, completion: .contentProcessed { [weak self] error in
            guard let error else { return }
            self?.queue.async {
                quipLog("engine_bridge_send_failed error=\(error)")
                self?.enqueuePending(data)
                self?.disconnectOnQueue(connection)
                self?.scheduleReconnect()
            }
        })
    }

    private func receiveNext(on connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) {
            [weak self, weak connection] data, _, complete, error in
            guard let self, let connection else { return }
            self.queue.async {
                guard self.connection === connection else { return }
                if let data, !data.isEmpty {
                    self.receiveBuffer.append(data)
                    self.drainMessages()
                }
                if let error {
                    quipLog("engine_bridge_receive_failed error=\(error)")
                    self.disconnectOnQueue(connection)
                    self.scheduleReconnect()
                } else if complete {
                    self.disconnectOnQueue(connection)
                    self.scheduleReconnect()
                } else {
                    self.receiveNext(on: connection)
                }
            }
        }
    }

    private func drainMessages() {
        while let newline = receiveBuffer.firstIndex(of: 0x0A) {
            let line = receiveBuffer[..<newline]
            receiveBuffer.removeSubrange(...newline)
            guard !line.isEmpty,
                  let object = try? JSONSerialization.jsonObject(with: line),
                  let message = object as? [String: Any]
            else {
                quipLog("engine_bridge_decode_failed")
                continue
            }
            if message["type"] as? String == "capture_accepted",
               let sessionID = message["session_id"] as? String,
               let generation = (message["generation"] as? NSNumber)?.uintValue,
               awaitingCaptureAcknowledgments[sessionID]?.generation == generation {
                awaitingCaptureAcknowledgments.removeValue(forKey: sessionID)
            }
            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .quipEngineBridgeMessage,
                    object: nil,
                    userInfo: message
                )
            }
        }
    }

    private func disconnectOnQueue(_ connection: NWConnection) {
        guard self.connection === connection else { return }
        connection.stateUpdateHandler = nil
        connection.cancel()
        self.connection = nil
        ready = false
        receiveBuffer.removeAll(keepingCapacity: true)
    }

    private func scheduleReconnect() {
        guard !reconnectScheduled else { return }
        reconnectScheduled = true
        queue.asyncAfter(deadline: .now() + 1) { [weak self] in
            guard let self else { return }
            self.reconnectScheduled = false
            self.connectOnQueue()
        }
    }

    private func launchQuipAppIfNeeded() {
        guard !attemptedAppLaunch else { return }
        attemptedAppLaunch = true
        DispatchQueue.main.async {
            let paths = ["/Applications/Quip Input.app", "/Applications/Quip.app"]
            guard let path = paths.first(where: FileManager.default.fileExists(atPath:)) else {
                quipLog("engine_bridge_app_not_installed")
                return
            }
            NSWorkspace.shared.openApplication(
                at: URL(fileURLWithPath: path),
                configuration: NSWorkspace.OpenConfiguration()
            ) { _, error in
                if let error {
                    quipLog("engine_bridge_app_launch_failed error=\(error)")
                } else {
                    quipLog("engine_bridge_app_launched path=\(path)")
                }
            }
        }
    }
}
