"""Keep Pillow's private native libraries private.

Pillow wheels include their own HarfBuzz/FreeType/libpng builds. PyInstaller
normally creates top-level symlinks to them, where they can replace the
Homebrew/MSYS libraries collected for Pango. The mixed ABI set crashes or
fails when WeasyPrint imports. Pango gets its own coherent dependency set at
the bundle root; Pillow continues to use the copies in ``PIL/.dylibs``.
"""

bindepend_symlink_suppression = [
    "**/PIL/.dylibs/libharfbuzz.0.dylib",
    "**/PIL/.dylibs/libfreetype.6.dylib",
    "**/PIL/.dylibs/libpng16.16.dylib",
]
