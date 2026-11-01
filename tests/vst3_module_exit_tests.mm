#import <Foundation/Foundation.h>
#include "VST3Host.h"
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <stdexcept>
#include <string>
#include <vector>

// Public C API, original fixture bundle only. No audio/render/editor calls.
// All modes return normally, including the missing-final-cleanup control.
static int checks=0;
static void require(bool condition,const char *message){++checks;if(!condition)throw std::runtime_error(message);}
static void trace(const char *message){
    const char *path=getenv("ST_VST3_FIXTURE_TRACE");
    if(path)if(FILE *file=fopen(path,"a")){fprintf(file,"%s\n",message);fclose(file);}
}
static std::vector<std::string> traceLines(){
    const char *path=getenv("ST_VST3_FIXTURE_TRACE");require(path&&*path,"private fixture trace is required");
    std::ifstream file(path);require(bool(file),"private trace can be inspected");
    std::vector<std::string> lines;std::string line;while(std::getline(file,line))lines.push_back(line);return lines;
}
static size_t count(const std::vector<std::string>& lines,const char *event){return std::count(lines.begin(),lines.end(),event);}
struct Discovered {std::string uid;int classes=0;};
static void found(const STVST3Class *item,void *context){auto& result=*static_cast<Discovered*>(context);result.uid=item->uid;++result.classes;}
struct ExpectedDiagnosticFailure : std::runtime_error {ExpectedDiagnosticFailure():std::runtime_error("original diagnostic failure fixture"){};};
struct Lifetime {
    STVST3Instance *first=nullptr,*second=nullptr;bool explicitCleanup=true,closed=false;
    explicit Lifetime(bool cleanup):explicitCleanup(cleanup){}
    void shutdown(){
        if(closed)return;closed=true;
        if(first){st_vst3_destroy(first);first=nullptr;}
        if(second){st_vst3_destroy(second);second=nullptr;}
        trace("host.instances.destroyed");
        if(explicitCleanup){st_vst3_release_unused_modules();trace("host.final.cleanup");}
        else trace("host.final.cleanup.omitted");
    }
    ~Lifetime(){shutdown();}
};
static void assertRetained(int destroyed){
    const auto lines=traceLines();
    require(count(lines,"bundle.entry")==1&&count(lines,"factory.create")==1,"scan and both instances must share exactly one module entry and factory");
    require(count(lines,"component.create")==2&&count(lines,"component.destroy")==size_t(destroyed),"live instance count matches the deliberate destruction point");
    require(!count(lines,"factory.destroy")&&!count(lines,"bundle.exit")&&!count(lines,"static.destroy"),"cleanup cannot release the module while an instance still lives");
}
static void noRender(STVST3Instance *instance){
    require(st_vst3_watch_parameter(instance,42),"original fixture exposes its known parameter");
    STVST3ParameterDiagnostics diagnostics{};
    require(st_vst3_parameter_diagnostics(instance,&diagnostics)&&!diagnostics.processes,"teardown test must not render audio");
}
int main(int argc,char **argv){@autoreleasepool{
    if(argc!=3){fprintf(stderr,"Usage: vst3-module-exit-tests PRIVATE_BUNDLE explicit-success|explicit-error|missing-cleanup\n");return 2;}
    const std::string mode=argv[2];
    if(mode!="explicit-success"&&mode!="explicit-error"&&mode!="missing-cleanup"){fprintf(stderr,"Unknown fixture mode\n");return 2;}
    try{
        Lifetime lifetime(mode!="missing-cleanup");char error[1024]{};Discovered discovered;
        require(st_vst3_scan_bundle(argv[1],found,&discovered,error,sizeof(error))==1,"original fixture scan succeeds");
        require(discovered.classes==1&&discovered.uid=="0123456789ABCDEF0123456789ABCDEF","scan returns only the original deterministic class");
        lifetime.first=st_vst3_create(argv[1],discovered.uid.c_str(),48000,128,error,sizeof(error));
        if(!lifetime.first)fprintf(stderr,"first create: %s\n",error);
        require(lifetime.first!=nullptr,"first original instance creates");
        lifetime.second=st_vst3_create(argv[1],discovered.uid.c_str(),48000,128,error,sizeof(error));
        if(!lifetime.second)fprintf(stderr,"second create: %s\n",error);
        require(lifetime.second!=nullptr,"second original instance creates");
        noRender(lifetime.first);noRender(lifetime.second);
        st_vst3_release_unused_modules();assertRetained(0);trace("host.two.live.retained");
        st_vst3_destroy(lifetime.first);lifetime.first=nullptr;
        st_vst3_release_unused_modules();assertRetained(1);
        require(st_vst3_parameter_count(lifetime.second)==1&&st_vst3_is_instrument(lifetime.second),"second instance remains usable after releasing the first");
        noRender(lifetime.second);trace("host.one.live.retained");
        if(mode=="explicit-error"){trace("host.expected.error");throw ExpectedDiagnosticFailure();}
        lifetime.shutdown();
        const auto lines=traceLines();require(count(lines,"component.destroy")==2,"both components are destroyed before process return");
        if(mode=="explicit-success"){
            require(count(lines,"factory.destroy")==1&&count(lines,"bundle.exit")==1,"explicit final cleanup releases factory and module before returning");
            require(!count(lines,"static.destroy"),"bundle statics still live during explicit factory cleanup");
            st_vst3_release_unused_modules();
            const auto repeated=traceLines();require(count(repeated,"factory.destroy")==1&&count(repeated,"bundle.exit")==1,"repeated final cleanup is idempotent");
        }else{
            require(!count(lines,"factory.destroy")&&!count(lines,"bundle.exit")&&!count(lines,"static.destroy"),"negative control leaves only the unused module cache for normal process exit");
        }
        trace("host.return.success");printf("PASS %s: %d in-process teardown checks; normal return, no rendering.\n",mode.c_str(),checks);return 0;
    }catch(const ExpectedDiagnosticFailure& error){
        const auto lines=traceLines();
        require(count(lines,"component.destroy")==2&&count(lines,"factory.destroy")==1&&count(lines,"bundle.exit")==1&&!count(lines,"static.destroy"),"exception unwind completes explicit cleanup while statics are alive");
        trace("host.return.expected-error");fprintf(stderr,"EXPECTED diagnostic error: %s\n",error.what());
        printf("PASS explicit-error: %d in-process teardown checks; expected diagnostic exit 42, no rendering.\n",checks);return 42;
    }catch(const std::exception& error){trace("host.unexpected.error");fprintf(stderr,"FAIL VST3 module exit: %s\n",error.what());return 1;}
}}
