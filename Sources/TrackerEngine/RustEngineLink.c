/* The engine's implementation is the Rust static library built from rust/tracker-engine
   (scripts/build-engine.sh). When the package links that library (see Package.swift), this file is the
   TrackerEngine target's only source: the target then just publishes the C headers in include/.
   In a C++ engine build (SUPERTRACKER_ENGINE=cpp) it is compiled alongside the C++ sources and adds nothing. */
typedef int st_engine_translation_unit_is_not_empty;
