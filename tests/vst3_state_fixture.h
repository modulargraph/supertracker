#pragma once
// Original in-memory component/controller shared by state boundary tests.
class StateComponent : public IComponent {
public:
    double value = .625;
    QUERY_ONE(IComponent) EMBEDDED_REFS
    tresult PLUGIN_API initialize(FUnknown *) override { return kResultOk; }
    tresult PLUGIN_API terminate() override { return kResultOk; }
    tresult PLUGIN_API getControllerClassId(TUID) override { return kNotImplemented; }
    tresult PLUGIN_API setIoMode(IoMode) override { return kResultOk; }
    int32 PLUGIN_API getBusCount(MediaType, BusDirection) override { return 0; }
    tresult PLUGIN_API getBusInfo(MediaType, BusDirection, int32, BusInfo&) override { return kInvalidArgument; }
    tresult PLUGIN_API getRoutingInfo(RoutingInfo&, RoutingInfo&) override { return kNotImplemented; }
    tresult PLUGIN_API activateBus(MediaType, BusDirection, int32, TBool) override { return kResultOk; }
    tresult PLUGIN_API setActive(TBool) override { return kResultOk; }
    tresult PLUGIN_API getState(IBStream *stream) override { return stream->write(&value, sizeof(value), nullptr); }
    tresult PLUGIN_API setState(IBStream *stream) override { int32 count = 0; return stream->read(&value, sizeof(value), &count) == kResultOk && count == sizeof(value) ? kResultOk : kResultFalse; }
};
class StateController : public IEditController {
public:
    enum Mode { complete, optionalEmpty, optionalPartial, failedEmpty, failedPartial, internalError } mode = complete;
    double value = .375;
    QUERY_ONE(IEditController) EMBEDDED_REFS
    tresult PLUGIN_API initialize(FUnknown *) override { return kResultOk; }
    tresult PLUGIN_API terminate() override { return kResultOk; }
    tresult PLUGIN_API setComponentState(IBStream *) override { return kResultOk; }
    tresult PLUGIN_API getState(IBStream *stream) override {
        if (mode == complete) return stream->write(&value, sizeof(value), nullptr);
        if (mode == optionalPartial || mode == failedPartial) stream->write(&value, 3, nullptr);
        if (mode == optionalEmpty || mode == optionalPartial) return kNotImplemented;
        return mode == internalError ? kInternalError : kResultFalse;
    }
    tresult PLUGIN_API setState(IBStream *stream) override { int32 count = 0; return stream->read(&value, sizeof(value), &count) == kResultOk && count == sizeof(value) ? kResultOk : kResultFalse; }
    int32 PLUGIN_API getParameterCount() override { return 0; }
    tresult PLUGIN_API getParameterInfo(int32, ParameterInfo&) override { return kInvalidArgument; }
    tresult PLUGIN_API getParamStringByValue(ParamID, ParamValue, String128) override { return kNotImplemented; }
    tresult PLUGIN_API getParamValueByString(ParamID, TChar *, ParamValue&) override { return kNotImplemented; }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID, ParamValue value) override { return value; }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID, ParamValue value) override { return value; }
    ParamValue PLUGIN_API getParamNormalized(ParamID) override { return value; }
    tresult PLUGIN_API setParamNormalized(ParamID, ParamValue normalized) override { value = normalized; return kResultOk; }
    tresult PLUGIN_API setComponentHandler(IComponentHandler *) override { return kResultOk; }
    IPlugView *PLUGIN_API createView(FIDString) override { return nullptr; }
};
