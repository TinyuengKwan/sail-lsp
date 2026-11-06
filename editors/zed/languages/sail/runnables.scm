; Runnables
;---------
; Generic runnable tags for Sail files.
; Bind concrete commands in `.zed/tasks.json`.

(source_file) @run
(#set! tag sail-check)

(source_file) @run
(#set! tag sail-build)

(source_file) @run
(#set! tag sail-run)
