// Command emfor-conformance-lookup is the Go lookup program for the shared
// conformance driver (tools/emfor-fixture, emfor-conformance):
// <name> <affinity-key or ""> <count>, printing "OK <address>" or
// "ERR <CODE> <message>" per lookup.
package main

import (
	"errors"
	"fmt"
	"os"
	"strconv"

	"github.com/vanillahsu/emfor/clients/go"
)

func main() {
	if len(os.Args) != 4 {
		fmt.Fprintln(os.Stderr, "usage: emfor-conformance-lookup <name> <affinity-key> <count>")
		os.Exit(2)
	}
	n, err := strconv.Atoi(os.Args[3])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	for i := 0; i < n; i++ {
		addr, err := emfor.Lookup(os.Args[1], os.Args[2])
		var e *emfor.Error
		switch {
		case err == nil:
			fmt.Println("OK " + addr)
		case errors.As(err, &e):
			fmt.Println("ERR " + e.Code.String() + " " + e.Message)
		default:
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	}
}
