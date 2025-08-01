(module
  (func $env.print (;0;) (import "env" "print") (param i32))
  (func $<start> (;1;) (export "<start>")
    i32.const 30
    call $env.print
    i32.const 20
    call $env.print
  )
)