// Prints the built-in effect catalogue as JSON, in the shape of the `builtin.effects` bridge command
// (BuiltinEffectsCatalogue.swift). Used by tests/effect_pages_tests.js to check the shared effect pages
// against the real native metadata.
#include "STBuiltinEffects.h"
#include <cstdio>
#include <string>

static std::string quote(const char *text) {
    std::string out = "\"";
    for (const char *c = text ? text : ""; *c; ++c) { if (*c == '"' || *c == '\\') out += '\\'; out += *c; }
    return out + "\"";
}

int main() {
    std::printf("[");
    for (uint32_t i = 0; i < st_builtin_effect_count(); ++i) {
        const auto *e = st_builtin_effect_info(i);
        std::printf("%s{\"id\":%s,\"name\":%s,\"category\":%s,\"sidechain\":%s,\"parameters\":[", i ? "," : "",
            quote((std::string("stfx:") + e->id).c_str()).c_str(), quote(e->name).c_str(), quote(e->category).c_str(),
            (e->flags & ST_BUILTIN_EFFECT_SIDECHAIN) ? "true" : "false");
        for (uint32_t p = 0; p < e->parameterCount; ++p) {
            const auto *q = st_builtin_effect_parameter_info(e->id, p);
            std::printf("%s{\"address\":%u,\"id\":%s,\"name\":%s,\"unit\":%s,\"min\":%.17g,\"max\":%.17g,\"defaultValue\":%.17g,"
                "\"logarithmic\":%s,\"integer\":%s,\"toggle\":%s,\"labels\":[", p ? "," : "", p, quote(q->id).c_str(), quote(q->name).c_str(),
                quote(q->unit).c_str(), q->minimum, q->maximum, q->defaultValue, (q->flags & ST_BUILTIN_PARAMETER_LOG) ? "true" : "false",
                (q->flags & ST_BUILTIN_PARAMETER_INTEGER) ? "true" : "false", (q->flags & ST_BUILTIN_PARAMETER_TOGGLE) ? "true" : "false");
            for (uint32_t l = 0; l < q->labelCount; ++l) std::printf("%s%s", l ? "," : "", quote(q->labels[l]).c_str());
            std::printf("]}");
        }
        std::printf("]}");
    }
    std::printf("]\n");
}
