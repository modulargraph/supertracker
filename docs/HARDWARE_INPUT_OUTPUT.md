# Hardware inputs, sampling and MIDI output

Open **Line inputs…** above the pattern grid (also available in the command palette).
Assign a mono channel or stereo pair to each track, enable the inputs you want to hear,
and apply. Several tracks can use different channels of the same audio interface at
once. Each input passes through that track's inserts and mixer controls. Existing
samples and instrument plug-ins remain on their tracks.

Select the interface in Audio & MIDI settings. To combine separate physical interfaces,
create a macOS Aggregate Device in Audio MIDI Setup and select it as the input device.
Saved routes remember the interface UID: a missing interface does not silently connect
a different source. The Line inputs dialog shows inactive routes and lets you explicitly
assign the current interface. Enabling hardware inputs requests microphone access when
needed. Opening a saved project does not trigger that permission request.

Open the **Recording** main tab (**⌘7**) and use its embedded capture form, or use **Record sample…** above the pattern grid / **⌘⇧R**. Every ordinary sample capture defaults to a new sampler instrument plus a new tracker lane; existing instruments are preserved. Replacement requires choosing the named replacement destination explicitly. For a selected live-input
track the recorder starts with that track's interface and channels, and defaults to a
new sampler instrument and track. This preserves the original live input and its
processing. The recorder also supports replacing a sample, recording a kit Sound,
and attaching audio clips or takes. A failed import retains the recording for retry.
Offline export cannot reproduce a live hardware signal: record it first. Track Freeze records it for you
(**Record and freeze**: one real-time pass captured at the compensated track strip; see `docs/TRACK_FREEZE_UX.md`).

For external instruments, choose **External MIDI** on the instrument page, select the
output port and MIDI channel, and enter notes normally. Playback and keyboard audition
send to the selected port even when MIDI input is disabled. A disconnected saved port
stays visible; choose its replacement explicitly. Stop/panic and route changes release
notes and pedals so a previous destination cannot keep sounding.

Automated coverage uses synthetic multichannel audio and private CoreMIDI destinations;
physical cabling, device drivers and audible latency still need a hardware listening check.
