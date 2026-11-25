Why this folder exists
======================

The x86_64-pc-windows-gnu Rust toolchain on this PC is missing one import
library that std links against: `bcryptprimitives`. Every fresh link of this
project failed with

    ld: cannot find -lbcryptprimitives

even for code that had never been touched, because `ProcessPrng` (the Windows
random source std uses to seed hashers) lives in that DLL.

`libbcryptprimitives.a` here was generated from the DLL Windows already has, so
nothing was downloaded and no toolchain files were changed:

    printf 'LIBRARY bcryptprimitives.dll\nEXPORTS\nProcessPrng\n' > bcryptprimitives.def
    dlltool -d bcryptprimitives.def -l libbcryptprimitives.a -D bcryptprimitives.dll

`dlltool` comes from MSYS2 (C:\msys64\mingw64\bin). `.cargo/config.toml` adds
this folder with `-L native=vendor/mingw` for the GNU target so the linker can
find it.

If you ever reinstall or update the GNU toolchain and it starts linking without
help, this folder plus the `[target.x86_64-pc-windows-gnu]` block in
`.cargo/config.toml` can be deleted.
