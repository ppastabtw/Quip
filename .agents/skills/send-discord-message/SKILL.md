---
name: send-discord-message
description: Send a Discord message through the signed-in Windows desktop app. Use when the user asks to message a Discord chat, channel, or person. Default to the Discord group chat named hackthe6ix unless the user specifies another destination.
---

# Send a Discord message

1. Read and follow the `computer-use:computer-use` skill before controlling Discord.
2. Use `hackthe6ix` as the destination unless the user specifies another chat, channel, or recipient.
3. Select Discord and its window only from values returned by Computer Use. Navigate to the destination and prepare the exact message without sending it.
4. Show the destination and message to the user. Request confirmation immediately before Send because this is representational communication.
5. After confirmation, send once, refresh the window state, and verify the message appears. Report success or the exact blocker.

Stop if Discord is signed out, the desktop is locked, or the destination is ambiguous.
