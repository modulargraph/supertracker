#import <Foundation/Foundation.h>
#include "VST2Host.h"
#include <climits>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <vector>

// Public host API only. The script supplies a private original fixture bundle;
// no installed plug-in discovery, editor, audio render or device calls occur.
static int checks=0;
static void require(bool condition,const char *message) {
    ++checks;
    if(!condition){std::fprintf(stderr,"FAIL VST2 bank state: %s\n",message);std::exit(1);}
}
static void append32(std::vector<uint8_t> &bytes,uint32_t value) {
    for(int shift=0;shift<32;shift+=8)bytes.push_back(uint8_t(value>>shift));
}
static uint32_t read32(const std::vector<uint8_t> &bytes,size_t at) {
    require(at+4<=bytes.size(),"state header field is present");
    uint32_t value=0;for(int i=0;i<4;++i)value|=uint32_t(bytes[at+i])<<(8*i);return value;
}
static std::vector<uint8_t> state(int32_t uniqueID,int32_t program,const std::vector<uint8_t> &bank) {
    std::vector<uint8_t> bytes{'S','T','V','2'};
    append32(bytes,1);append32(bytes,uint32_t(uniqueID));append32(bytes,1);
    append32(bytes,uint32_t(program));append32(bytes,uint32_t(bank.size()));
    bytes.insert(bytes.end(),bank.begin(),bank.end());return bytes;
}
static std::vector<uint8_t> capture(STVST2Instance *host) {
    char error[512]{};void *data=nullptr;size_t size=0;
    const int result=st_vst2_copy_state(host,&data,&size,error,sizeof(error));
    if(!result)std::fprintf(stderr,"copy error: %s\n",error);
    require(result&&data&&size>=24,"capture publishes complete host state");
    std::vector<uint8_t> bytes(static_cast<uint8_t *>(data),static_cast<uint8_t *>(data)+size);
    st_vst2_free(data);return bytes;
}
static void assertCaptured(STVST2Instance *host,int32_t uniqueID,int32_t program,const std::vector<uint8_t> &bank) {
    const auto bytes=capture(host);
    require(bytes.size()==24+bank.size(),"saved payload length is exact");
    require(std::memcmp(bytes.data(),"STV2",4)==0&&read32(bytes,4)==1,"save retains STV2 version 1");
    require(read32(bytes,8)==uint32_t(uniqueID)&&read32(bytes,12)==1,"save retains exact identity and bank state kind");
    require(read32(bytes,16)==uint32_t(program),"save retains the selected bank program");
    require(read32(bytes,20)==bank.size(),"save records exact bank byte count");
    require(std::vector<uint8_t>(bytes.begin()+24,bytes.end())==bank,"program selection never rewrites opaque bank bytes");
}
static void restore(STVST2Instance *host,const std::vector<uint8_t> &bytes) {
    char error[512]{};const int result=st_vst2_restore_state(host,bytes.data(),bytes.size(),error,sizeof(error));
    if(!result)std::fprintf(stderr,"restore error: %s\n",error);
    require(result!=0,"valid bank envelope restores successfully");
}
int main(int argc,char **argv) { @autoreleasepool {
    require(argc==2,"one private original fixture path is required");
    char error[512]{};
    std::unique_ptr<STVST2Instance,decltype(&st_vst2_destroy)> host(
        st_vst2_create(argv[1],0,48000,128,error,sizeof(error)),st_vst2_destroy);
    if(!host)std::fprintf(stderr,"create error: %s\n",error);
    require(bool(host),"the original private fixture loads directly");
    STVST2Info info{};require(st_vst2_info(host.get(),&info)!=0,"fixture metadata is available");
    require(info.unique_id==0x44624238&&info.program_chunks&&st_vst2_program_count(host.get())==8,
            "the selected bundle is the original eight-program bank fixture");
    require(st_vst2_parameter_count(host.get())==6,"fixture dispatch counters are exposed");
    const std::vector<uint8_t> bank{0x42,0x4b,0,0xff,0x80,7,6,5,4,3,2,1,0,19,201,0,33};
    for(int32_t program:{3,0,7}) {
        require(st_vst2_set_program(host.get(),6)!=0,"fixture begins on a different selected program");
        const float beforePrograms=st_vst2_parameter_value(host.get(),3);
        const float beforeChunks=st_vst2_parameter_value(host.get(),5);
        restore(host.get(),state(info.unique_id,program,bank));
        require(st_vst2_program(host.get())==program,"saved selection must be restored after the bank chunk resets it to zero");
        require(st_vst2_parameter_value(host.get(),0)==float(program+1)/16.f,"parameter cache must reflect the restored program");
        require(st_vst2_parameter_value(host.get(),3)==beforePrograms+1,"valid program, including zero, is dispatched exactly once");
        require(st_vst2_parameter_value(host.get(),4)==0,"no invalid program dispatch occurs");
        require(st_vst2_parameter_value(host.get(),5)==beforeChunks+1,"bank chunk is loaded exactly once");
        assertCaptured(host.get(),info.unique_id,program,bank);
    }
    for(int32_t program:{-1,8,INT_MAX}) {
        require(st_vst2_set_program(host.get(),6)!=0,"invalid-index fixture starts from another program");
        const float beforePrograms=st_vst2_parameter_value(host.get(),3);
        const float beforeChunks=st_vst2_parameter_value(host.get(),5);
        restore(host.get(),state(info.unique_id,program,bank));
        require(st_vst2_program(host.get())==0,"out-of-range saved selection must leave the bank's default program");
        require(st_vst2_parameter_value(host.get(),0)==1.f/16.f,"invalid index must retain the bank-default parameter value");
        require(st_vst2_parameter_value(host.get(),3)==beforePrograms,"invalid program must never reach effSetProgram");
        require(st_vst2_parameter_value(host.get(),4)==0,"fixture sees no rejected program dispatch");
        require(st_vst2_parameter_value(host.get(),5)==beforeChunks+1,"invalid program does not suppress valid bank loading");
        assertCaptured(host.get(),info.unique_id,0,bank);
    }
    restore(host.get(),state(info.unique_id,3,bank));
    const auto roundTrip=capture(host.get());
    const std::vector<uint8_t> alternate{0,9,8,7,6,5,4,3,2,1,0xff};
    restore(host.get(),state(info.unique_id,7,alternate));
    assertCaptured(host.get(),info.unique_id,7,alternate);
    restore(host.get(),roundTrip);
    require(st_vst2_program(host.get())==3&&st_vst2_parameter_value(host.get(),0)==.25f,"saved bank restores both program and observable parameter after intervening state");
    assertCaptured(host.get(),info.unique_id,3,bank);
    require(st_vst2_parameter_value(host.get(),2)==0,"test must not render audio frames");
    std::printf("PASS VST2 bank state: %d checks; program restored after bank reset, first/last valid indices, invalid indices never dispatched, exact bank bytes and program retained across save/restore; original fixture only, no audio rendering.\n",checks);
} }
