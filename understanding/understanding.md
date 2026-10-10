We're building peek, that is a silicon app with native mac frontend for carbons, and a cli interface for silicons.

This is a app designed for quick comms between carbons & silicons running locally. This is a voice first app for carbons, but supports text as well.

Use Cases:
- A clean up silicon running locally, and it just needs to ask if a file is imp, or should be cleaned. Simple question that no one will come back to again. 
- A music silicon that acts like a DJ and while playing songs, it just wants to show the cover art, name of the song, artist and year of publishing for 10secs on screen.
- A reminder silicon that pops up and reminds of a certain thing.

Whenever there doesn't need to be a history and doesn't need a complete chat, esp for background works running that needs to give quick information or ask a quick question.

## Visuals
Inside ./peek all-positions.md you'll see the 8 available positions. A silicon can occupy exactly one of them. If its taken, then it must choose something else.

When the silicon triggers its peek via cli, the bubble reserved for it appears from the sides like it was hiding behind it withe same mac's animation (wiht a little bit of a bounce).

Inside each position, there's a circular visual area (drawable) and a informaiton arc. When choosing a position among 8, a silicon can register its drawing for the visual area that is displayed. This is a js file. check ./visual.md for how it works technically.

Visual is set once and changed rarely, and then silicon can use it to send information.
Drawing gets access to the voice that is being spoken and can respond/react/visually move based on it.

Drawing area is not resizeable.

## CLI
`peek send --speak "..." --show "{...}"` to speak and show something. show is a json that accepts type text (max 160char), image+caption (text optional, max 50char). and show can have max 3 elements. max-width per element is 1/3 of total arc space after padding between elements. its like flex justify center. image location is some local address.

`peek send --speak "..." --ask "{...}"` to speak and ask a question. ask should be self suffient. it is of the type {question (max 80chars), type (text, single_choice, multiple_choice, slider, range), type specific things}. question can not be an image, options can have be images. slider and range will be use the entire arc space and be movable. question is its own arc above, and is curved. answers are sent to silicon over ting.

--show, --ask are both mutually exclusive. and --speak, --show and --ask are optional. atleast one is needed.

`peek register drawing ./logo.js` registers and uses the logo/animation when peek send is used.

`peek register side ...` pick or change a side.

`peek unregister` to remove itself and hold no position to itself.

`peek ...` authentication cli needed by Silicon Accounts.

## Inputs
Each visual is attached to a silicon, and has a keyboard shortcut for it. def: cmd+{1,2,3...8} 1 is top center and moving clockwise.

Below each visual are 2 buttons with icons: mic and a keyboard. mic starts to listen to the user, and keyboard opens it into a typing area. when either one is clicked, it expads in X and takes up the entire space. in case of the voice, it shows the live waveform of what's being recorded. in case of keyboard, it expands to the same size, just to type. and hiting enter sends it.

## Interactions
When silicon sends a --show with --speak, it auto slides back after the speach is over. but there is also a {down-arrow} to close the visual. single click on the {down-arrow} only closes the visual and slides it back down. double click on it stops the audio as well.

after pressing a keyboard shortcut to slide in a peek, a keyboard shortcut \ can be used to enter into voice mode, or can start typing to enter into text mode. esc to cancel it and slide it back.

when silicon tries to run `peek send` commands without setting a drawing, or location among 8, it show throw an erorr asking to do that first.

## Auth & Server
peek will be authenticated using Silicon Accounts so it can distinguishes between different silicons, send msg over ting, and store information per silicon as needed. All information / send history is kept locally. peek is a very local app and barely uses global for much. It does have a server to keep the app secret and register ting, store drawing on server etc etc.

## Settings
Make 2 modes: Normal mode and compact mode. in compact mode, the arc is not an arc, its just a line, and visual becomes very small and goes to the left, and the show/ask is displayed on the right. it takes less space in compact.

There should be a simulation mode where there are toggles of various options like peek position, speak, show, ask, etc etc, and then clicking simulate shows what it looks like when silicon triggers it. It has some sample values it simulates with.

## Style
Very much with bg blur, glass effect, and almost native apple feel with text pills, images having borders, etc and thing bending to the information arc.

Keep a track of what its drawing over, and use shades of white and black for bg based on what will be the best for that background to stand out.

for dark mode and light mode, that information is also sent to the drawing area so it can use it.

## Codebase
Make the mac app native, for backend write it in rust. same with CLI, make it in rust and ship the binary.
Make a frontend explaining what peek does and how it works. make it in solid js. keep the style similar to the app itself.
I am a certified apple developer, so sign the app using it.
Publish everything on Silicon Apps.
Learn how to use Silicon Accounts, Silicon Apps, Ting, and how everything will work inside silicon-stemcell
(github.com/teamofsilicons/...) and each one has its own documentation as well on their websites.
use @chrome for whatever you need.
FOR TTS AND STT, use deepgram directly.