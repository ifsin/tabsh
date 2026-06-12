package main

import (
	"encoding/json"
	"log"
	"net/http"
	"os"
	"sync/atomic"
	"time"

	"github.com/google/uuid"
	"github.com/gorilla/websocket"
)

const (
	s2cPtyOut   = byte(0x00)
	s2cReattach = byte(0x01)
	s2cState    = byte(0x02)
)

const (
	c2sInput  = byte(0x00)
	c2sResize = byte(0x01)
	c2sInit   = byte(0x02)
	c2sQuit   = byte(0x03)
	c2sClear  = byte(0x04)
)

const (
	writeWait  = 10 * time.Second
	pongWait   = 60 * time.Second
	pingPeriod = 30 * time.Second
)

var upgrader = websocket.Upgrader{
	CheckOrigin:     func(r *http.Request) bool { return true },
	Subprotocols:    []string{"tabsh"},
	ReadBufferSize:  4096,
	WriteBufferSize: 32768,
}

type initMsg struct {
	SessionID string `json:"sessionId"`
	Cols      uint16 `json:"cols"`
	Rows      uint16 `json:"rows"`
	Cwd       string `json:"cwd"`
	AppID     string `json:"appId"`
	Cmd       string `json:"cmd"`
}

type resizeMsg struct {
	Cols uint16 `json:"cols"`
	Rows uint16 `json:"rows"`
}

var activeClients int32

func makeWSHandler(cfg *Config) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if cfg.Server.MaxClients > 0 {
			cur := int(atomic.AddInt32(&activeClients, 1))
			if cur > cfg.Server.MaxClients {
				atomic.AddInt32(&activeClients, -1)
				http.Error(w, "max clients reached", http.StatusServiceUnavailable)
				return
			}
			defer atomic.AddInt32(&activeClients, -1)
		}

		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			log.Printf("ws upgrade: %v", err)
			return
		}
		defer conn.Close()

		var sess *Session

		sendCh := make(chan []byte, 256)
		quit := make(chan struct{})
		defer close(quit)

		go func() {
			ping := time.NewTicker(pingPeriod)
			defer ping.Stop()
			for {
				select {
				case <-quit:
					return
				case msg, ok := <-sendCh:
					if !ok {
						return
					}
					conn.SetWriteDeadline(time.Now().Add(writeWait))
					if err := conn.WriteMessage(websocket.BinaryMessage, msg); err != nil {
						return
					}
				case <-ping.C:
					conn.SetWriteDeadline(time.Now().Add(writeWait))
					if err := conn.WriteMessage(websocket.PingMessage, nil); err != nil {
						return
					}
				}
			}
		}()

		conn.SetReadDeadline(time.Now().Add(pongWait))
		conn.SetPongHandler(func(string) error {
			conn.SetReadDeadline(time.Now().Add(pongWait))
			return nil
		})

		for {
			_, msg, err := conn.ReadMessage()
			if err != nil {
				break
			}
			if len(msg) == 0 {
				continue
			}
			cmd, payload := msg[0], msg[1:]

			switch cmd {
			case c2sInit:
				var init initMsg
				if err := json.Unmarshal(payload, &init); err != nil {
					log.Printf("bad init: %v", err)
					continue
				}
				if init.SessionID == "" {
					init.SessionID = uuid.NewString()
				}
				if init.Cols == 0 {
					init.Cols = 80
				}
				if init.Rows == 0 {
					init.Rows = 24
				}

				existing := sessionFind(init.SessionID)
				if existing != nil {
					existing.attach(conn)
					existing.mu.Lock()
					existing.send = sendCh
					existing.mu.Unlock()
					sess = existing
					sess.resize(init.Cols, init.Rows)
					sendCh <- []byte{s2cReattach}
					existing.log.Replay(func(body []byte) {
						frame := make([]byte, 1+len(body))
						frame[0] = s2cPtyOut
						copy(frame[1:], body)
						sendCh <- frame
					})
				} else {
					app := findApp(cfg, init.AppID)
					cwd := init.Cwd
					if cwd == "" {
						cwd = app.Cwd
					}
					if cwd == "" {
						cwd, _ = os.UserHomeDir()
					}
					ptmx, err := spawnPTY(app, init.Cols, init.Rows, cwd, cfg.Server.TerminalType)
					if err != nil {
						errMsg, _ := json.Marshal(map[string]string{"error": "spawn_failed", "detail": err.Error()})
						sendCh <- append([]byte{s2cState}, errMsg...)
						continue
					}

					sl, err := openSessionLog(init.SessionID, init.Cols, init.Rows)
					if err != nil {
						log.Printf("log open: %v", err)
					}

					sess = &Session{
						id:          init.SessionID,
						ptmx:        ptmx,
						conn:        conn,
						send:        sendCh,
						cols:        init.Cols,
						rows:        init.Rows,
						log:         sl,
						done:        make(chan struct{}),
						closeSignal: parseSignal(cfg.Server.Signal),
					}
					sessionStore(sess)
					go ptyReadLoop(sess)

					cwdMsg, _ := json.Marshal(map[string]string{"cwd": cwd})
					sendCh <- append([]byte{s2cState}, cwdMsg...)

					if init.Cmd != "" {
						ptmx.Write([]byte(init.Cmd + "\n"))
					}
				}

			case c2sInput:
				if sess != nil && len(payload) > 0 {
					sess.ptmx.Write(payload)
				}

			case c2sResize:
				if sess == nil {
					continue
				}
				var rm resizeMsg
				if err := json.Unmarshal(payload, &rm); err == nil && rm.Cols > 0 && rm.Rows > 0 {
					sess.resize(rm.Cols, rm.Rows)
				}

			case c2sQuit:
				if sess != nil {
					sess.close()
					sessionDelete(sess.id)
				}
				return

			case c2sClear:
				if sess != nil {
					sess.log.Delete()
					sess.log, _ = openSessionLog(sess.id, sess.cols, sess.rows)
					sess.ptmx.Write([]byte("\x1b[H\x1b[2J"))
				}
			}
		}

		if sess != nil {
			sess.detachConn()
		}
	}
}

func ptyReadLoop(sess *Session) {
	buf := make([]byte, 4096)
	for {
		select {
		case <-sess.done:
			return
		default:
		}
		n, err := sess.ptmx.Read(buf)
		if n > 0 {
			sess.osc.Feed(buf[:n], func(body string) {
				if cmd, ok := parseCmdOSC(body); ok {
					sess.setCurrentCommand(cmd)
				}
			})
			data := make([]byte, 1+n)
			data[0] = s2cPtyOut
			copy(data[1:], buf[:n])
			sess.log.Append(buf[:n])
			sess.writeToClient(data)
		}
		if err != nil {
			exitMsg, _ := json.Marshal(map[string]string{"error": "shell_exited"})
			sess.writeToClient(append([]byte{s2cState}, exitMsg...))
			sess.close()
			sessionDelete(sess.id)
			return
		}
	}
}

func findApp(cfg *Config, appID string) *AppConfig {
	for i := range cfg.Apps {
		if cfg.Apps[i].ID == appID {
			return &cfg.Apps[i]
		}
	}
	if len(cfg.Apps) > 0 {
		return &cfg.Apps[0]
	}
	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "/bin/sh"
	}
	return &AppConfig{ID: "shell", Command: shell}
}
