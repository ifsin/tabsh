package main

import (
	"encoding/binary"
	"log"
	"os"
	"path/filepath"
)

const (
	logDir     = "/tmp/tabsh-sessions"
	logMaxSize = 1 * 1024 * 1024
	logHdrSize = 8
)

type SessionLog struct {
	path string
	f    *os.File
	size int64
}

func openSessionLog(id string, cols, rows uint16) (*SessionLog, error) {
	if err := os.MkdirAll(logDir, 0700); err != nil {
		return nil, err
	}
	path := filepath.Join(logDir, id+".log")

	existing := false
	if fi, err := os.Stat(path); err == nil {
		existing = fi.Size() >= logHdrSize
	}

	f, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return nil, err
	}

	sl := &SessionLog{path: path, f: f}

	if !existing {
		hdr := make([]byte, logHdrSize)
		binary.LittleEndian.PutUint32(hdr[0:], uint32(cols))
		binary.LittleEndian.PutUint32(hdr[4:], uint32(rows))
		if _, err := f.Write(hdr); err != nil {
			f.Close()
			return nil, err
		}
	}

	fi, _ := f.Stat()
	sl.size = fi.Size()
	if _, err := f.Seek(0, 2); err != nil {
		f.Close()
		return nil, err
	}
	return sl, nil
}

func (sl *SessionLog) Append(data []byte) {
	if sl == nil || sl.f == nil {
		return
	}
	if _, err := sl.f.Write(data); err != nil {
		log.Printf("log write: %v", err)
		return
	}
	sl.size += int64(len(data))
	if sl.size > logMaxSize {
		sl.compact()
	}
}

func (sl *SessionLog) compact() {
	hdr := make([]byte, logHdrSize)
	if _, err := sl.f.ReadAt(hdr, 0); err != nil {
		return
	}
	bodySize := sl.size - logHdrSize
	keep := bodySize / 2
	start := logHdrSize + (bodySize - keep)

	body := make([]byte, keep)
	if _, err := sl.f.ReadAt(body, start); err != nil {
		return
	}

	tmp := sl.path + ".tmp"
	t, err := os.Create(tmp)
	if err != nil {
		return
	}
	if _, err := t.Write(hdr); err != nil || func() bool { _, e := t.Write(body); return e != nil }() {
		t.Close()
		os.Remove(tmp)
		return
	}
	t.Close()
	sl.f.Close()
	os.Rename(tmp, sl.path)

	sl.f, _ = os.OpenFile(sl.path, os.O_RDWR|os.O_APPEND, 0600)
	sl.size = logHdrSize + keep
}

func (sl *SessionLog) Replay(fn func([]byte)) {
	if sl == nil || sl.f == nil {
		return
	}
	sl.f.Sync()
	fi, err := sl.f.Stat()
	if err != nil || fi.Size() <= logHdrSize {
		return
	}
	body := make([]byte, fi.Size()-logHdrSize)
	if _, err := sl.f.ReadAt(body, logHdrSize); err != nil {
		return
	}
	fn(body)
}

func (sl *SessionLog) Close() {
	if sl != nil && sl.f != nil {
		sl.f.Close()
	}
}

func (sl *SessionLog) Delete() {
	if sl != nil {
		os.Remove(sl.path)
	}
}
