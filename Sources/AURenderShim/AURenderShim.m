#import "AURenderShim.h"

static AUAudioUnitStatus missingRenderFunction(void *context, AudioUnitRenderActionFlags *flags, const AudioTimeStamp *timestamp,
    AUAudioFrameCount frames, NSInteger bus, AudioBufferList *output, const AURenderEvent *events, const void *pull) {
    return kAudioUnitErr_Uninitialized;
}

@implementation STAURenderUnit {
    STAURenderFunction _function;
    void *_context;
    BOOL _direct;
}

- (void)setRendersDirectly:(BOOL)direct {
    _direct = direct;
}

- (AURenderBlock)renderBlock {
    if (!_direct) return [super renderBlock];
    STAURenderFunction function = _function ? _function : missingRenderFunction;
    void *context = _context;
    return ^AUAudioUnitStatus(AudioUnitRenderActionFlags *flags, const AudioTimeStamp *timestamp, AUAudioFrameCount frames,
                              NSInteger bus, AudioBufferList *output, AURenderPullInputBlock pull) {
        return function(context, flags, timestamp, frames, bus, output, NULL, (__bridge const void *)pull);
    };
}

- (void)setRenderFunction:(STAURenderFunction)function context:(void *)context {
    _function = function;
    _context = context;
}

- (AUInternalRenderBlock)internalRenderBlock {
    STAURenderFunction function = _function ? _function : missingRenderFunction;
    void *context = _context;
    return ^AUAudioUnitStatus(AudioUnitRenderActionFlags *flags, const AudioTimeStamp *timestamp, AUAudioFrameCount frames,
                              NSInteger bus, AudioBufferList *output, const AURenderEvent *events, AURenderPullInputBlock pull) {
        return function(context, flags, timestamp, frames, bus, output, events, (__bridge const void *)pull);
    };
}
@end

AUAudioUnitStatus st_au_pull(const void *pull, AudioUnitRenderActionFlags *flags, const AudioTimeStamp *timestamp,
                             AUAudioFrameCount frames, NSInteger bus, AudioBufferList *data) {
    if (!pull) return kAudioUnitErr_NoConnection;
    return ((__bridge AURenderPullInputBlock)pull)(flags, timestamp, frames, bus, data);
}
