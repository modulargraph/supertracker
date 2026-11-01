# VST3 SDK source

SuperTracker's VST3 host uses Steinberg's official VST3 SDK interfaces. The host
implementation is in `Sources/VST3Host`; the application interface remains
AppKit/WebKit. No VSTGUI or JUCE interface is used.

Source: https://github.com/steinbergmedia/vst3sdk

Pinned SDK version: 3.8.1, build 84.

| Repository | Revision |
| --- | --- |
| vst3sdk | `3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96` |
| base | `fcf9da0bd27a16f7f03773a3a39822f28f5c8477` |
| pluginterfaces | `4f547e8e102b47de4a8b8aaf343c73b700786372` |
| public.sdk | `586dc5e6c8012c3e4b01c79389375cbe96bdb1da` |
| cmake | `054c9143cbb8d47fc4694e473f2ee3b4d951a8f5` |

The SDK and initialized modules include their MIT license. The build script copies
the SDK license into the application bundle's `Contents/Resources/Licenses` folder.
The SDK documentation, tutorials, and VSTGUI submodules are not needed to build
SuperTracker and have not been downloaded.
