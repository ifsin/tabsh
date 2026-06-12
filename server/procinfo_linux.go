//go:build linux

package main

import (
	"bytes"
	"os"
	"strconv"

	"golang.org/x/sys/unix"
)

func foregroundPID(ptmx *os.File) (int, error) {
	return unix.IoctlGetInt(int(ptmx.Fd()), unix.TIOCGPGRP)
}

func processArgv(pid int) ([]string, error) {
	data, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/cmdline")
	if err != nil {
		return nil, err
	}
	parts := bytes.Split(bytes.TrimRight(data, "\x00"), []byte{0})
	argv := make([]string, 0, len(parts))
	for _, p := range parts {
		if len(p) > 0 {
			argv = append(argv, string(p))
		}
	}
	return argv, nil
}
