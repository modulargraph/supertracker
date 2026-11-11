# Hardware inputs, sampling and MIDI output

Open **Line inputs…** above the pattern grid (also available in the command palette), or
use **Live input** on the selected track in the mixer. Assign a mono channel or stereo
pair to each track, enable the inputs you want to hear, and apply. Several tracks can use
different channels of the same audio interface at once. Each input enters its track ahead
of the inserts: it passes through that track's inserts, fader, sends and meters, and is
part of what is recorded from the master. Live inputs are heard without pressing Play.
**Monitor** off keeps a route but leaves it out of the track (for listening through the
interface instead). Existing samples and instrument plug-ins remain on their tracks.
A route is part of the project: changing it is an ordinary edit with Undo, and a saved
project brings its routes back.

Select the interface in Audio & MIDI settings; the choice is remembered. To combine
separate physical interfaces, create a macOS Aggregate Device in Audio MIDI Setup and
select it as the input device (routes are then assigned on the aggregate). Saved routes
remember their interface by the name the system lists it under: a route whose interface
is not the selected input is paused, with the reason shown, and never connected to a
different source. It runs again when its interface is the input. The Line inputs dialog
shows paused routes and lets you explicitly assign the current interface. The input
device is opened when a route needs it, which includes opening a project that has
enabled routes; macOS asks for microphone access the first time.

**Hardware return latency.** A hardware instrument that follows the song answers late:
its note leaves when the block is heard, and its audio comes back through the input. A
track with a live input therefore has latency in the mixer's delay compensation (one
output block, the input latency the device reports, and one block of cushion), and the
other tracks wait for it, so the return lines up with them. Input and output of one
interface share a clock. Two separate devices drift apart: the input then has a short
gap or skip every few minutes. Use one interface, or an aggregate device, for inputs
that matter.

Open the **Recording** main tab (**⌘7**) and use its embedded capture form, or use **Record sample…** above the pattern grid / **⌘⇧R**. Every ordinary sample capture defaults to a new sampler instrument plus a new tracker lane; existing instruments are preserved. Replacement requires choosing the named replacement destination explicitly. For a selected live-input
track the recorder starts with that track's interface and channels, and defaults to a
new sampler instrument and track. This preserves the original live input and its
processing. The recorder also supports replacing a sample, recording a kit Sound,
and attaching audio clips or takes. A failed import retains the recording for retry.
Offline export cannot reproduce a live hardware signal: record it first. Track Freeze records it for you
(**Record and freeze**: the song played once in real time and captured at the compensated track strip; see `docs/TRACK_FREEZE_UX.md`).
A frozen track plays its print and no longer blocks export.

For external instruments, choose **External MIDI** on the instrument page, select the
output port and MIDI channel, and enter notes normally. Playback and keyboard audition
send to the selected port even when MIDI input is disabled. A disconnected saved port
stays visible; choose its replacement explicitly. Stop/panic and route changes release
notes and pedals so a previous destination cannot keep sounding.

Automated coverage uses the null audio device with a generated input and in-memory MIDI
ports. No audio input device has been opened: physical cabling, device drivers, the
microphone permission on macOS, drift between two devices and audible latency still
need a check on hardware.
