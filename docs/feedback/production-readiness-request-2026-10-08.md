# Production readiness request

Saved verbatim from the owner's message on 2026-10-08. This is a requirements reference, not a completion report.

Fix the edges with that is possible.
When loading the application i expect all the files to instantly show up after it has been indexed but it isnt. its taking a while to load now. it didnt used to. I timed it at 1 minute 15 seconds on the circular loading screen. Far too slow for us. Any kind of sorting does this. Size, name, Modified, Path ect. It must be 3 seconds max to the user.
Can we replace the Default top bar with our own so we dont need to think about it. mix it into the design all we need to do is put the minimize Maximise and Close button to the  top right of the current application and it'll deal with it.
I also noticed there is not a easy way to search from a specific point. for example today i wanted to only search my NVME drives but there was not a easy option to do so that was obvious to me.
Clicking the Neutra software text at the bottom right should send you to [https://neutra.software](https://neutra.software)
We dont need the status dot for searching. And we shouldnt need the "searching" text anyways as it should be almost instant to the user.
Add to the background service a thing where if you do Control/command + K anywhere it brings up like a spotlight style searchbar to bring up anyfile you want instantly, this is more convenient than having to open the whole app for one file.
The Catagory boxes look ugly. lets just go with the icon Plus the catagory name seperated by a verticle line.
It said in the index menu "index updated 4 hours ago" which shouldnt be correct because i've changed files since then. it should say "a few seconds ago" at most.
I should be able to interact with neutrasearch like a normal file explorer, drag things in and out, open and close, delete, restore (control + z), right click open with, Rename. ect. the whole shaboodle.
Currently there is no right click menu in the TreeMap.
I'd like a few different choices of treemap style, I like the one we have now but thats folder centric. I'd like one that does the windirstat style where it shows you it in file formats biggest and smallest, along with a Physics Ball graph. Everything is connected by their hirarchy with bigger balls being bigger and smaller balls being smaller. I'd like to be able to interact with these balls and boxes the exact same how i would with a file within the normal "explorer like" menu.&#x20;
The indexed space menu on the left side needs right click menus for continuity.
I've probably missed some things but if somthing obvious comes up please just do it. The UI needs cleaning up still.
We need to get this Production Ready.
