# Vendor binaries

Programs the recordings need that Debian does not package, kept out of git.

`p4/p4` and `p4/p4d`: the Perforce client and server for Linux x86_64, from
Perforce's Helix Core Server archive (`helix-core-server.tgz`, the
`bin.linux26x86_64` build; `p4 p4d` extracted from it). Only the Perforce
tapes use them; without them the image builds as before and a full
`run.sh` leaves those tapes out. On an arm64 machine they run through the amd64 loader the image adds
(Rosetta under OrbStack, or qemu-user with binfmt elsewhere).
