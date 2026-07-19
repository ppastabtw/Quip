#!/usr/bin/env swift

import Carbon

let sourceID = "com.hackthe6ix.inputmethod.QuipSwift.en" as CFString
let filter = [kTISPropertyInputSourceID as String: sourceID] as CFDictionary
let sources = TISCreateInputSourceList(filter, false).takeRetainedValue()
    as! [TISInputSource]

guard sources.count == 1 else {
    fatalError("expected one Quip Native input source, found \(sources.count)")
}

let source = sources[0]
let enableStatus = TISEnableInputSource(source)
guard enableStatus == noErr else {
    fatalError("TISEnableInputSource failed with status \(enableStatus)")
}

let selectStatus = TISSelectInputSource(source)
guard selectStatus == noErr else {
    fatalError("TISSelectInputSource failed with status \(selectStatus)")
}

print("Enabled and selected Quip Native")
