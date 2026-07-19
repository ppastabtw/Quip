#import "client_shim.h"

@protocol QuipDynamicTextClient
- (NSRange)selectedRange;
- (NSRect)firstRectForCharacterRange:(NSRange)range
                          actualRange:(NSRangePointer)actualRange;
- (id)attributesForCharacterIndex:(NSUInteger)index
              lineHeightRectangle:(NSRectPointer)rect;
- (void)insertText:(id)text replacementRange:(NSRange)replacementRange;
@end

NSRange QuipClientSelectedRange(id client) {
  if (client != nil && [client respondsToSelector:@selector(selectedRange)]) {
    return [(id<QuipDynamicTextClient>)client selectedRange];
  }
  return NSMakeRange(NSNotFound, 0);
}

NSRect QuipClientFirstRect(id client, NSRange range) {
  if (client != nil &&
      [client respondsToSelector:@selector(firstRectForCharacterRange:actualRange:)]) {
    NSRange actual = NSMakeRange(NSNotFound, 0);
    return [(id<QuipDynamicTextClient>)client firstRectForCharacterRange:range
                                                     actualRange:&actual];
  }
  if (client != nil &&
      [client respondsToSelector:@selector(attributesForCharacterIndex:lineHeightRectangle:)]) {
    NSRect rect = NSZeroRect;
    [(id<QuipDynamicTextClient>)client attributesForCharacterIndex:range.location
                                               lineHeightRectangle:&rect];
    return rect;
  }
  return NSZeroRect;
}

BOOL QuipClientInsertText(id client, NSString *text, NSRange replacementRange) {
  if (client != nil &&
      [client respondsToSelector:@selector(insertText:replacementRange:)]) {
    [(id<QuipDynamicTextClient>)client insertText:text
                                 replacementRange:replacementRange];
    return YES;
  }
  return NO;
}
