// Linked only into the standalone NKS diagnostic, never the application target.
#include "vst3_signal_guard.h"
extern "C" int st_install_nks_diagnostic_guard() {
    return VST3Diagnostic::installFatalSignalGuard() ? 1 : 0;
}
