# Unix credential delivery

The Python kernel receives approved directory handles over a Unix socket using
`SCM_RIGHTS`. Control-message length fields use the platform's libc types;
ARM64 musl uses narrower fields than GNU libc.

Linux first attempts `open_tree(OPEN_TREE_CLONE)` to clamp parent traversal at
the approved root. When unavailable, it falls back to `O_PATH | O_DIRECTORY`.
macOS uses `O_RDONLY | O_DIRECTORY`, since it has neither Linux API. Directory
handles without a mount clone do not clamp parent traversal; filesystem access
remains subject to the process sandbox policy.

CI checks the kernel for both musl and macOS architectures and runs the
descriptor-delivery test on macOS. The Linux-only mount-clone test stays on
Linux, where that behavior exists.
