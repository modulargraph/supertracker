#ifndef SUPERTRACKER_AU_RENDER_SHIM_H
#define SUPERTRACKER_AU_RENDER_SHIM_H
#import <AudioToolbox/AudioToolbox.h>

NS_ASSUME_NONNULL_BEGIN

/* Render callback for an in-process Audio Unit implemented in Swift. `pull` is the host's
   AURenderPullInputBlock, unretained and valid only during the call; pass it to st_au_pull.
   Swift closures that receive an AURenderPullInputBlock bridge it into a heap-allocated
   Swift closure on every render call; an Objective-C render block hands it over as-is. */
typedef AUAudioUnitStatus (*STAURenderFunction)(void *_Nullable context, AudioUnitRenderActionFlags *flags,
    const AudioTimeStamp *timestamp, AUAudioFrameCount frames, NSInteger bus, AudioBufferList *output,
    const AURenderEvent *_Nullable events, const void *_Nullable pull);

/* Base class whose internalRenderBlock calls an STAURenderFunction. Set the function before
   render resources are allocated (normally in init); the context is not retained. */
@interface STAURenderUnit : AUAudioUnit
- (void)setRenderFunction:(STAURenderFunction)function context:(void *_Nullable)context;
/* For host-internal units with no parameters, scheduled events or render observers: the
   public renderBlock calls the render function directly, skipping AUAudioUnit's event
   schedule and observer bookkeeping (a host-time read and lookups on every render of
   every node). Render observers added to such a unit are never called. */
- (void)setRendersDirectly:(BOOL)direct;
@end

/* Calls the pull block handed to an STAURenderFunction. Returns kAudioUnitErr_NoConnection without one. */
AUAudioUnitStatus st_au_pull(const void *_Nullable pull, AudioUnitRenderActionFlags *flags,
    const AudioTimeStamp *timestamp, AUAudioFrameCount frames, NSInteger bus, AudioBufferList *data);

NS_ASSUME_NONNULL_END
#endif
