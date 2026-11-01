#include <cstdint>
#include <cstdio>
#include <cstring>
#ifdef ORIGINAL_BANK_PROGRAMS
#include <vector>
#endif

// Original deterministic VST2 fixture; no device, editor, file or thread access.
// ABI declarations are local and do not use or distribute a vendor SDK.
struct Effect;
using Callback = intptr_t (*)(Effect *, int32_t, int32_t, intptr_t, void *, float);
using Process = void (*)(Effect *, float **, float **, int32_t);
struct Effect {
    int32_t magic; Callback dispatcher; Process process;
    void (*setParameter)(Effect *,int32_t,float); float (*getParameter)(Effect *,int32_t);
    int32_t numPrograms,numParams,numInputs,numOutputs,flags;
    intptr_t reserved1,reserved2; int32_t initialDelay,realQualities,offQualities;
    float ioRatio; void *object,*user; int32_t uniqueID,version;
    Process processReplacing; void *processDoubleReplacing; char future[56];
};
struct State {
    Effect effect{}; float gain=.25f, failCapture=0, renderedFrames=0; Callback host=nullptr;
#ifdef ORIGINAL_BANK_PROGRAMS
    int32_t selectedProgram=0, setProgramCalls=0, invalidProgramCalls=0, setChunkCalls=0;
    std::vector<uint8_t> bank{0x42,0x41,0x4e,0x4b,0,0xff,7,0x80};
#endif
#ifdef ORIGINAL_MIDI_NOTES
    uint8_t held[16][128]{}; int32_t ignoredPanics=0; // A synth that ignores CC 120/123, as many VST2 instruments do.
#endif
};
static State *state(Effect *effect) { return static_cast<State *>(effect->object); }
static intptr_t dispatch(Effect *effect,int32_t op,int32_t,intptr_t size,void *ptr,float) {
    auto s=state(effect);
    switch(op) {
    case 1: delete s; return 1;
#ifdef ORIGINAL_BANK_PROGRAMS
    case 2:
        ++s->setProgramCalls;
        if(size<0||size>=effect->numPrograms){++s->invalidProgramCalls;return 0;}
        s->selectedProgram=static_cast<int32_t>(size);
        s->gain=float(s->selectedProgram+1)/16.f;
        return 1;
    case 3: return s->selectedProgram;
#endif
    case 6: if(ptr)std::strcpy(static_cast<char *>(ptr),"");return 1;
    case 7: if(ptr)std::snprintf(static_cast<char *>(ptr),32,"%.3f",s->gain);return 1;
    case 8: if(ptr)std::strcpy(static_cast<char *>(ptr),"Original gain");return 1;
#ifdef ORIGINAL_BANK_PROGRAMS
    case 23:
        if(s->failCapture>.5f)return -1;
        *static_cast<void **>(ptr)=s->bank.data();return static_cast<intptr_t>(s->bank.size());
    case 24:
        if(size<=0||size>4*1024*1024||!ptr)return 0;
        s->bank.assign(static_cast<uint8_t *>(ptr),static_cast<uint8_t *>(ptr)+size);
        ++s->setChunkCalls;s->selectedProgram=0;s->gain=1.f/16.f;return 0;
#else
    case 23: if(s->failCapture>.5f)return -1;*static_cast<void **>(ptr)=&s->gain;return sizeof(float);
    case 24: if(size==sizeof(float)&&ptr)std::memcpy(&s->gain,ptr,sizeof(float));return 0;
#endif
#ifdef ORIGINAL_MIDI_NOTES
    case 25: {
        struct Events { int32_t count; intptr_t reserved; uint8_t *events[1]; };
        auto events=static_cast<Events *>(ptr);
        for(int32_t n=0;events&&n<events->count;++n){const uint8_t *midi=events->events[n]+24;int type=midi[0]&0xf0,channel=midi[0]&15,note=midi[1]&127;
            if(type==0x90&&midi[2])s->held[channel][note]=1;else if(type==0x80||type==0x90)s->held[channel][note]=0;else if(type==0xb0&&(midi[1]==120||midi[1]==123))++s->ignoredPanics;}
        return 1;
    }
#endif
    case 35: return effect->numInputs ? 1:2;
    case 45: case 48: if(ptr)std::strcpy(static_cast<char *>(ptr),"Original Debug VST2");return 1;
    case 47: if(ptr)std::strcpy(static_cast<char *>(ptr),"SuperTracker Tests");return 1;
    case 49: return 1;
    case 58: return 2400;
    default:return 0;
    }
}
static void process(Effect *effect,float **,float **output,int32_t count) {
    for(int32_t i=0;i<count;i++)output[0][i]=output[1][i]=state(effect)->gain;
    state(effect)->renderedFrames+=count;
    state(effect)->host(effect,42,0,0,nullptr,0);
}
extern "C" __attribute__((visibility("default"))) Effect *VSTPluginMain(Callback host) {
    auto s=new State;auto &e=s->effect;e.magic=0x56737450;e.dispatcher=dispatch;e.process=process;
    s->host=host;
    e.processReplacing=process;e.setParameter=[](Effect *e,int32_t i,float v){if(i==0)state(e)->gain=v;else if(i==1)state(e)->failCapture=v;};
    e.getParameter=[](Effect *e,int32_t i){
#ifdef ORIGINAL_BANK_PROGRAMS
        if(i==3)return float(state(e)->setProgramCalls);
        if(i==4)return float(state(e)->invalidProgramCalls);
        if(i==5)return float(state(e)->setChunkCalls);
#endif
#ifdef ORIGINAL_MIDI_NOTES
        if(i==3){int held=0;for(auto &channel:state(e)->held)for(auto note:channel)held+=note;return float(held);}
#endif
        return i==0?state(e)->gain:i==1?state(e)->failCapture:state(e)->renderedFrames;
    };
    e.numPrograms=1;e.numParams=3;e.numInputs=0;e.numOutputs=2;e.flags=(1<<4)|(1<<5)|(1<<8);
    e.object=s;e.uniqueID=0x44624731;e.version=1;
#ifdef ORIGINAL_EFFECT
    e.uniqueID=0x44624732;e.numInputs=2;e.flags &= ~(1<<8);
#endif
#ifdef ORIGINAL_BANK_PROGRAMS
    e.uniqueID=0x44624238;e.numPrograms=8;e.numParams=6;s->gain=1.f/16.f;
#endif
#ifdef ORIGINAL_MIDI_NOTES
    e.uniqueID=0x44624d4e;e.numParams=4;
#endif
    return &e;
}
