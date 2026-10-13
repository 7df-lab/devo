# Goal activation while a turn is finishing

Pausing a goal cancels its continuation and waits a bounded time for termination.
A Python cell can take longer to finish cancellation, event delivery and merge.
An explicit resume arriving in that interval must remain pending even though the
active-turn registry still owns the interrupted turn.

Goal activation now records that request on the active execution. After merge,
cleanup removes the execution and consumes the request under the same registry
lock, then post-turn scheduling checks the current goal and queued user input.
A request arriving after cleanup takes the ordinary idle admission path. Normal
Escape interruption does not itself request another goal continuation.

A pause response that finishes after a concurrent resume mirrors and returns the
latest host goal state, avoiding an older paused snapshot replacing that resume.
Turn execution and approval continuation use atomic full cleanup after merge;
the previous separate interrupt-handle cleanup is no longer needed there.

The integration regression runs a real Python cell and a bounded slow subscriber
to hold terminal delivery open. Baseline interrupts the first turn but loses the
resume; the fix admits a second continuation, completes the goal from Python and
terminates normally. The test gates on the cell's filesystem marker rather than
an execution preview or an arbitrary race-establishing sleep.
