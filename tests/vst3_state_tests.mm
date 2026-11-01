// Original fake component/controller: no installed plugin or audio device needed.
#include "../Sources/VST3Host/VST3Host.mm"
#include <cassert>
#include <cstdio>

#include "vst3_state_fixture.h"
class SynchronizingController final : public StateController {
public:
    tresult result = kResultOk;
    bool readValue = false;
    tresult PLUGIN_API setComponentState(IBStream *stream) override {
        if (readValue) { int32 read = 0; assert(stream->read(&value,sizeof(value),&read) == kResultOk && read == sizeof(value)); }
        return result;
    }
};
int main() { @autoreleasepool {
    StateComponent component; StateController controller;
    auto host = std::make_unique<STVST3Instance>(); host->component = &component; host->controller = &controller;
    copyText(host->uid, "0123456789ABCDEF0123456789ABCDEF");
    char error[512]{}; void *bytes = nullptr; size_t count = 0;
    for (auto mode : {StateController::complete, StateController::optionalEmpty}) {
        controller.mode = mode; component.value = .625; controller.value = .375;
        assert(st_vst3_copy_state(host.get(), &bytes, &count, error, sizeof(error)));
        const size_t expected = 56 + sizeof(double) + (mode == StateController::complete ? sizeof(double) : 0);
        assert(bytes && count == expected);
        component.value = controller.value = .125;
        assert(st_vst3_restore_state(host.get(), bytes, count, error, sizeof(error)));
        assert(component.value == .625);
        assert(controller.value == (mode == StateController::complete ? .375 : .125));
        st_vst3_free(bytes);
    }
    for (auto mode : {StateController::failedEmpty, StateController::failedPartial, StateController::internalError, StateController::optionalPartial}) {
        controller.mode = mode; bytes = reinterpret_cast<void *>(1); count = 99;
        if (st_vst3_copy_state(host.get(), &bytes, &count, error, sizeof(error)) || bytes || count) {
            fprintf(stderr, "FAIL controller mode %d incorrectly published incomplete state\n", int(mode));
            if (bytes && bytes != reinterpret_cast<void *>(1)) st_vst3_free(bytes);
            return 1;
        }
        assert(strstr(error, "controller") && strstr(error, "complete"));
    }
    host->controller = nullptr;
    assert(st_vst3_copy_state(host.get(), &bytes, &count, error, sizeof(error)) && count == 56 + sizeof(double));
    st_vst3_free(bytes);
    // SDK base controllers may omit component synchronization, but explicit
    // controller errors cannot be presented as a successful coherent restore.
    StateComponent syncComponent; SynchronizingController syncController;
    auto syncHost = std::make_unique<STVST3Instance>(); syncHost->component = &syncComponent; syncHost->controller = &syncController;
    copyText(syncHost->uid,"5653544E6924486D6173736976652078"); syncHost->module = std::make_shared<Module>(); syncHost->module->bundleVersion = "1.7.1";
    const double incoming = .47;
    for (auto result : {kResultOk,kNotImplemented}) {
        syncController.result = result; syncController.readValue = result == kResultOk;
        assert(st_vst3_restore_component_state(syncHost.get(),&incoming,sizeof(incoming),error,sizeof(error)));
        assert(!syncHost->failedStateRestore && syncComponent.value == incoming);
        if (syncController.readValue) assert(syncController.value == incoming);
        assert(st_vst3_copy_state(syncHost.get(),&bytes,&count,error,sizeof(error))); st_vst3_free(bytes);
    }
    for (auto result : {kResultFalse,kInternalError,kInvalidArgument}) for (bool partiallyRead : {false,true}) {
        syncController.result = result; syncController.readValue = partiallyRead;
        assert(!st_vst3_restore_component_state(syncHost.get(),&incoming,sizeof(incoming),error,sizeof(error)));
        assert(syncHost->failedStateRestore && strstr(error,"synchronization"));
        bytes = reinterpret_cast<void *>(1); count = 99;
        assert(!st_vst3_copy_state(syncHost.get(),&bytes,&count,error,sizeof(error)) && !bytes && !count);
        uint64_t token = 99; assert(!st_vst3_begin_state_capture(syncHost.get(),1,&token,error,sizeof(error)) && !token);
        // An explicit valid restoration makes this clean instance usable again.
        syncController.result = kResultOk; syncController.readValue = true;
        assert(st_vst3_restore_component_state(syncHost.get(),&incoming,sizeof(incoming),error,sizeof(error)));
        assert(st_vst3_copy_state(syncHost.get(),&bytes,&count,error,sizeof(error))); st_vst3_free(bytes);
    }
    printf("PASS VST3 complete/optional controller state roundtrip; failed/partial saves publish no state; optional synchronization accepted, explicit controller-sync failures reject restore/capture.\n");
} }
