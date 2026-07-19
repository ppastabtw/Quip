import AppKit
import InputMethodKit

@MainActor
class QuipNativeInputController: IMKInputController {
    private static let idleDelay = 0.22
    private static let maxDraftUTF16 = 80

    private lazy var sessionID = {
        let pointer = UInt(bitPattern: Unmanaged.passUnretained(self).toOpaque())
        return "\(ProcessInfo.processInfo.processIdentifier)-\(pointer)"
    }()
    private var activeClient: AnyObject?
    private var draft = ""
    private var draftStart = 0
    private var draftEnd = 0
    private var generation: UInt = 0
    private var offeredDestinationID: String?
    private var offeredGeneration: UInt?
    private var observingBridge = false

    override func inputText(_ string: String!, client sender: Any!) -> Bool {
        let input = string ?? ""
        quipLog("text_event utf16_length=\(input.utf16.count)")
        guard let sender else {
            quipLog("text_event_no_client")
            return false
        }
        let client = sender as AnyObject
        activeClient = client
        let selected = QuipClientSelectedRange(client)
        guard selected.location != NSNotFound else {
            quipLog("text_event_no_selected_range client=\(String(describing: type(of: client)))")
            return false
        }

        if let digit = input.first?.wholeNumberValue,
           (1...5).contains(digit),
           input.count == 1,
           let destinationID = offeredDestinationID {
            QuipEngineBridge.shared.select(
                sessionID: sessionID,
                destinationID: destinationID,
                index: digit - 1
            )
            quipLog("engine_candidate_selected index=\(digit - 1)")
            return true
        }

        if offeredDestinationID != nil {
            QuipEngineBridge.shared.dismiss(sessionID: sessionID)
            offeredDestinationID = nil
            offeredGeneration = nil
        }
        guard !input.isEmpty,
              !input.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else {
            reset(at: selected.location, dismiss: true)
            return false
        }

        if draft.isEmpty || selected.length != 0 || selected.location != draftEnd {
            reset(at: selected.location, dismiss: true)
        }
        draft.append(input)
        draftEnd = selected.location + input.utf16.count
        trimDraftIfNeeded()
        generation &+= 1
        let scheduledGeneration = generation

        DispatchQueue.main.asyncAfter(deadline: .now() + Self.idleDelay) { [weak self] in
            guard let self, self.generation == scheduledGeneration,
                  let client = self.activeClient else { return }
            self.captureIfStable(client: client, generation: scheduledGeneration)
        }
        return false
    }

    override func didCommand(by selector: Selector!, client sender: Any!) -> Bool {
        guard let destinationID = offeredDestinationID else { return false }
        let command = selector.map(NSStringFromSelector) ?? ""
        switch command {
        case "insertTab:", "insertBacktab:":
            QuipEngineBridge.shared.accept(
                sessionID: sessionID,
                destinationID: destinationID
            )
            quipLog("engine_candidate_accepted command=\(command)")
            return true
        case "moveLeft:", "moveUp:":
            QuipEngineBridge.shared.move(
                sessionID: sessionID,
                destinationID: destinationID,
                delta: -1
            )
            return true
        case "moveRight:", "moveDown:":
            QuipEngineBridge.shared.move(
                sessionID: sessionID,
                destinationID: destinationID,
                delta: 1
            )
            return true
        case "cancelOperation:":
            QuipEngineBridge.shared.dismiss(sessionID: sessionID)
            offeredDestinationID = nil
            offeredGeneration = nil
            quipLog("engine_offer_dismissed command=\(command)")
            return true
        default:
            return false
        }
    }

    override func activateServer(_ sender: Any!) {
        observeBridgeIfNeeded()
        QuipEngineBridge.shared.connect()
        activeClient = sender.map { $0 as AnyObject }
        let selected = activeClient.map(QuipClientSelectedRange)
            ?? NSRange(location: 0, length: 0)
        reset(at: selected.location == NSNotFound ? 0 : selected.location, dismiss: false)
        quipLog("server_activated session=\(sessionID) client=\(String(describing: type(of: sender as Any)))")
    }

    override func deactivateServer(_ sender: Any!) {
        reset(at: 0, dismiss: true)
        activeClient = nil
        quipLog("server_deactivated session=\(sessionID)")
    }

    override func inputControllerWillClose() {
        NotificationCenter.default.removeObserver(self)
        observingBridge = false
        reset(at: 0, dismiss: true)
        activeClient = nil
    }

    private func captureIfStable(client: AnyObject, generation: UInt) {
        let selected = QuipClientSelectedRange(client)
        guard selected.length == 0, selected.location == draftEnd, !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            reset(at: selected.location == NSNotFound ? draftEnd : selected.location, dismiss: true)
            return
        }
        var caret = QuipClientFirstRect(
            client,
            NSRange(location: selected.location, length: 0)
        )
        if !isUsableCaret(caret), selected.location > 0 {
            caret = QuipClientFirstRect(
                client,
                NSRange(location: selected.location - 1, length: 1)
            )
            caret.origin.x += caret.width
            caret.size.width = 1
        }
        if !isUsableCaret(caret) {
            quipLog("engine_capture_using_accessibility_caret_fallback")
        }
        QuipEngineBridge.shared.capture(
            sessionID: sessionID,
            generation: generation,
            draft: draft,
            caret: caret
        )
        quipLog("engine_capture_sent generation=\(generation) draft_utf16=\(draft.utf16.count)")
    }

    private func isUsableCaret(_ caret: NSRect) -> Bool {
        guard caret.origin.x.isFinite,
              caret.origin.y.isFinite,
              caret.width.isFinite,
              caret.height.isFinite,
              caret.height >= 4,
              caret.height <= 200,
              caret.width >= 0,
              caret.width <= 200
        else { return false }
        let bounds = NSRect(
            x: caret.minX,
            y: caret.minY,
            width: max(caret.width, 1),
            height: max(caret.height, 1)
        )
        return NSScreen.screens.contains { screen in
            screen.frame.insetBy(dx: -64, dy: -64).intersects(bounds)
        }
    }

    private func observeBridgeIfNeeded() {
        guard !observingBridge else { return }
        observingBridge = true
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleBridgeMessage(_:)),
            name: .quipEngineBridgeMessage,
            object: nil
        )
    }

    @objc private func handleBridgeMessage(_ notification: Notification) {
        guard let message = notification.userInfo,
              message["session_id"] as? String == sessionID,
              let type = message["type"] as? String else { return }
        let messageGeneration = (message["generation"] as? NSNumber)?.uintValue

        switch type {
        case "settled":
            guard messageGeneration == generation,
                  let destinationID = message["destination_id"] as? String else { return }
            let offered = message["offered"] as? Bool ?? false
            if offered {
                offeredDestinationID = destinationID
                offeredGeneration = messageGeneration
                quipLog("engine_offer_ready generation=\(generation) destination=\(destinationID)")
            } else {
                reset(at: draftEnd, dismiss: false)
                quipLog("engine_offer_skipped generation=\(messageGeneration ?? 0)")
            }
        case "commit":
            guard messageGeneration == offeredGeneration,
                  message["destination_id"] as? String == offeredDestinationID,
                  let text = message["text"] as? String,
                  let client = activeClient else { return }
            let selected = QuipClientSelectedRange(client)
            guard selected.length == 0, selected.location == draftEnd else {
                quipLog("engine_commit_rejected reason=selection_moved")
                reset(at: selected.location == NSNotFound ? draftEnd : selected.location, dismiss: true)
                return
            }
            let replacement = NSRange(
                location: draftStart,
                length: draftEnd.saturatingSubtracting(draftStart)
            )
            guard QuipClientInsertText(client, text, replacement) else {
                quipLog("engine_commit_rejected reason=insert_unavailable")
                return
            }
            quipLog("engine_commit_applied location=\(replacement.location) length=\(replacement.length) text_utf16=\(text.utf16.count)")
            reset(at: draftStart + text.utf16.count, dismiss: false)
        case "dismissed":
            if message["destination_id"] as? String == offeredDestinationID {
                offeredDestinationID = nil
                offeredGeneration = nil
                reset(at: draftEnd, dismiss: false)
            }
        case "error":
            quipLog("engine_bridge_error message=\(message["message"] ?? "unknown")")
        default:
            break
        }
    }

    private func trimDraftIfNeeded() {
        let units = draft.utf16.count
        guard units > Self.maxDraftUTF16 else { return }
        var keptUnits = 0
        var start = draft.endIndex
        for index in draft.indices.reversed() {
            let character = draft[index]
            let width = String(character).utf16.count
            if keptUnits + width > Self.maxDraftUTF16 { break }
            keptUnits += width
            start = index
        }
        draft = String(draft[start...])
        draftStart = draftEnd - keptUnits
    }

    private func reset(at location: Int, dismiss: Bool) {
        if dismiss, offeredDestinationID != nil {
            QuipEngineBridge.shared.dismiss(sessionID: sessionID)
        }
        generation &+= 1
        draft.removeAll(keepingCapacity: true)
        draftStart = location
        draftEnd = location
        offeredDestinationID = nil
        offeredGeneration = nil
    }
}

private extension Int {
    func saturatingSubtracting(_ other: Int) -> Int {
        self >= other ? self - other : 0
    }
}
