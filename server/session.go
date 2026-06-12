package main

import (
	"encoding/json"
	"log"
	"os"
	"sync"
	"syscall"
	"time"

	"github.com/creack/pty"
	"github.com/gorilla/websocket"
)

const detachTimeout = 10 * time.Second

type Session struct {
	id          string
	ptmx        *os.File
	conn        *websocket.Conn
	send        chan []byte
	cols        uint16
	rows        uint16
	log         *SessionLog
	detach      *time.Timer
	mu          sync.Mutex
	done        chan struct{}
	osc         oscScanner
	cmd         string
	cmdActive   bool
	lastProc    string
	polling     bool
	closeSignal syscall.Signal
}

var sessions sync.Map // string → *Session

func sessionFind(id string) *Session {
	v, ok := sessions.Load(id)
	if !ok {
		return nil
	}
	return v.(*Session)
}

func sessionStore(s *Session) {
	sessions.Store(s.id, s)
}

func sessionDelete(id string) {
	sessions.Delete(id)
}

func killAllSessions() {
	sessions.Range(func(k, v any) bool {
		v.(*Session).close()
		return true
	})
}

func (s *Session) attach(conn *websocket.Conn) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.detach != nil {
		s.detach.Stop()
		s.detach = nil
	}
	s.conn = conn
}

func (s *Session) detachConn() {
	s.mu.Lock()
	s.conn = nil
	s.send = nil
	s.mu.Unlock()
	s.detach = time.AfterFunc(detachTimeout, func() {
		log.Printf("session %s timed out", s.id)
		s.close()
		sessionDelete(s.id)
	})
}

func (s *Session) resize(cols, rows uint16) {
	s.cols = cols
	s.rows = rows
	pty.Setsize(s.ptmx, &pty.Winsize{Cols: cols, Rows: rows})
}

func (s *Session) writeToClient(msg []byte) {
	s.mu.Lock()
	ch := s.send
	s.mu.Unlock()
	if ch == nil {
		return
	}
	select {
	case ch <- msg:
	default:
	}
}

func (s *Session) sendState(state map[string]string) {
	body, _ := json.Marshal(state)
	s.writeToClient(append([]byte{s2cState}, body...))
}

func (s *Session) close() {
	select {
	case <-s.done:
	default:
		close(s.done)
	}
	if s.ptmx != nil {
		if s.closeSignal != 0 {
			if pid, err := foregroundPID(s.ptmx); err == nil && pid > 0 {
				syscall.Kill(pid, s.closeSignal)
			}
		}
		s.ptmx.Close()
	}
	if s.log != nil {
		s.log.Close()
	}
}
