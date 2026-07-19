#import <AppKit/AppKit.h>
#import <InputMethodKit/InputMethodKit.h>
#import <stdint.h>

extern bool quip_imk_handle_event(uintptr_t controller_id, NSEvent *event, id client);
extern void quip_imk_activate(uintptr_t controller_id, id client);
extern void quip_imk_deactivate(uintptr_t controller_id);
extern void quip_imk_close(uintptr_t controller_id);

@interface QuipInputController : IMKInputController
@end

@implementation QuipInputController

- (instancetype)initWithServer:(IMKServer *)server
                       delegate:(id)delegate
                         client:(id)client {
  return [super initWithServer:server delegate:delegate client:client];
}

- (NSUInteger)recognizedEvents:(id)sender {
  return NSEventMaskKeyDown;
}

- (BOOL)handleEvent:(NSEvent *)event client:(id)client {
  return quip_imk_handle_event((uintptr_t)self, event, client);
}

- (void)activateServer:(id)client {
  quip_imk_activate((uintptr_t)self, client);
}

- (void)deactivateServer:(id)client {
  quip_imk_deactivate((uintptr_t)self);
}

- (void)inputControllerWillClose {
  quip_imk_close((uintptr_t)self);
}

@end

// Rust calls this symbol so the linker pulls the Objective-C object file out
// of its static archive, preserving QuipInputController in __objc_classlist.
void quip_imk_shim_force_link(void) {}
