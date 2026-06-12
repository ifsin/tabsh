//go:build darwin

package main

import (
	"os"
	"os/exec"
	"strconv"
	"strings"

	"golang.org/x/sys/unix"
)

func foregroundPID(ptmx *os.File) (int, error) {
	return unix.IoctlGetInt(int(ptmx.Fd()), unix.TIOCGPGRP)
}

func processArgv(pid int) ([]string, error) {
	out, err := exec.Command("ps", "-p", strconv.Itoa(pid), "-o", "command=").Output()
	if err != nil {
		return nil, err
	}
	return strings.Fields(strings.TrimSpace(string(out))), nil
}
