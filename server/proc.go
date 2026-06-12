package main

import (
	"path/filepath"
	"strings"
	"time"
)

func (s *Session) startProcPolling() {
	s.mu.Lock()
	if s.polling {
		s.mu.Unlock()
		return
	}
	s.polling = true
	s.mu.Unlock()

	go func() {
		ticker := time.NewTicker(400 * time.Millisecond)
		defer ticker.Stop()
		defer func() {
			s.mu.Lock()
			s.polling = false
			s.mu.Unlock()
		}()

		for {
			select {
			case <-s.done:
				return
			case <-ticker.C:
			}

			s.mu.Lock()
			active := s.cmdActive
			s.mu.Unlock()
			if !active {
				return
			}

			pid, err := foregroundPID(s.ptmx)
			if err != nil || pid <= 0 {
				continue
			}
			argv, err := processArgv(pid)
			if err != nil || len(argv) == 0 {
				continue
			}
			proc := strings.Join(argv, " ")
			if proc == "" {
				continue
			}

			s.mu.Lock()
			if proc == s.lastProc {
				s.mu.Unlock()
				continue
			}
			s.lastProc = proc
			s.mu.Unlock()

			s.sendState(map[string]string{"proc": filepath.Base(argv[0])})
			s.resolveCommandIcon(proc)
		}
	}()
}
