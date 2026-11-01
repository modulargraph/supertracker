#import <Foundation/Foundation.h>
#include "VST2Host.h"
#include <cstdio>
#include <cstdlib>
#include <memory>

// Public host API with a private original fixture that ignores CC 120/123, like
// many VST2 instruments. Timing-event epochs and AU reset() rely on that panic,
// so the host must turn it into explicit note-offs. No device or editor is used.
static int checks=0;
static void require(bool condition,const char *message){++checks;if(!condition){std::fprintf(stderr,"FAIL VST2 MIDI panic: %s\n",message);std::exit(1);}}
static float left[64],right[64];
static void run(STVST2Instance *host,std::initializer_list<STVST2MIDI> midi){
    require(st_vst2_process(host,nullptr,nullptr,left,right,64,midi.begin(),uint32_t(midi.size()),nullptr,0,nullptr)!=0,"fixture block renders");
}
static int held(STVST2Instance *host){st_vst2_refresh_parameters(host);return int(st_vst2_parameter_value(host,3));}
int main(int argc,char **argv){ @autoreleasepool {
    require(argc==2,"one private original fixture path is required");
    char error[512]{};
    std::unique_ptr<STVST2Instance,decltype(&st_vst2_destroy)> host(st_vst2_create(argv[1],0,48000,64,error,sizeof error),st_vst2_destroy);
    if(!host)std::fprintf(stderr,"create error: %s\n",error);
    require(bool(host)&&st_vst2_prepare(host.get(),48000,64,error,sizeof error),"fixture loads and starts");
    run(host.get(),{{0,0x90,60,100},{5,0x93,64,90},{9,0x93,67,90},{10,0x83,67,0}});
    require(held(host.get())==2,"fixture tracks sounding notes");
    run(host.get(),{{3,0xb0,123,0}});
    require(held(host.get())==1,"CC 123 ends this channel's held notes even when the instrument ignores it");
    run(host.get(),{{0,0xb3,120,0}});
    require(held(host.get())==0,"CC 120 ends the remaining channel's held notes");
    run(host.get(),{{0,0x90,62,100},{0,0x9f,70,100}});
    require(held(host.get())==2,"new notes sound after a panic");
    require(st_vst2_reset(host.get())!=0,"reset succeeds while active");
    require(held(host.get())==0,"AU reset releases every held note");
    require(st_vst2_is_active(host.get())!=0,"reset leaves the plug-in processing");
    run(host.get(),{{0,0x90,60,100}});
    require(held(host.get())==1,"processing resumes after reset");
    std::printf("PASS VST2 MIDI panic (%d checks)\n",checks);
} return 0; }
