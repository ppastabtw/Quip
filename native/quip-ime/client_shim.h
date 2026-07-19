#import <AppKit/AppKit.h>

NSRange QuipClientSelectedRange(id client);
NSRect QuipClientFirstRect(id client, NSRange range);
BOOL QuipClientInsertText(id client, NSString *text, NSRange replacementRange);
