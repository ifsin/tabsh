package main

import (
	"strings"
)

type oscScanner struct {
	state int
	buf   []byte
}

const (
	oscGround = iota
	oscEsc
	oscBody
	oscBodyEsc
)

func (s *oscScanner) Feed(data []byte, onOSC func(string)) {
	for _, b := range data {
		switch s.state {
		case oscGround:
			if b == 0x1b {
				s.state = oscEsc
			}
		case oscEsc:
			if b == ']' {
				s.buf = s.buf[:0]
				s.state = oscBody
			} else if b == 0x1b {
				s.state = oscEsc
			} else {
				s.state = oscGround
			}
		case oscBody:
			switch b {
			case 0x07:
				onOSC(string(s.buf))
				s.state = oscGround
			case 0x1b:
				s.state = oscBodyEsc
			default:
				if len(s.buf) < 16<<10 {
					s.buf = append(s.buf, b)
				} else {
					s.state = oscGround
					s.buf = s.buf[:0]
				}
			}
		case oscBodyEsc:
			if b == '\\' {
				onOSC(string(s.buf))
				s.state = oscGround
			} else {
				if len(s.buf) < 16<<10 {
					s.buf = append(s.buf, 0x1b, b)
					s.state = oscBody
				} else {
					s.state = oscGround
					s.buf = s.buf[:0]
				}
			}
		}
	}
}

func parseCmdOSC(body string) (string, bool) {
	if !strings.HasPrefix(body, "777;cmd;") {
		return "", false
	}
	cmd, ok := percentDecode(body[len("777;cmd;"):])
	if !ok {
		return "", false
	}
	return cmd, true
}

func percentDecode(s string) (string, bool) {
	var b strings.Builder
	b.Grow(len(s))
	for i := 0; i < len(s); i++ {
		if s[i] != '%' {
			b.WriteByte(s[i])
			continue
		}
		if i+2 >= len(s) {
			b.WriteByte(s[i])
			continue
		}
		hi, ok1 := fromHex(s[i+1])
		lo, ok2 := fromHex(s[i+2])
		if !ok1 || !ok2 {
			b.WriteByte(s[i])
			continue
		}
		b.WriteByte((hi << 4) | lo)
		i += 2
	}
	return b.String(), true
}

func fromHex(c byte) (byte, bool) {
	switch {
	case c >= '0' && c <= '9':
		return c - '0', true
	case c >= 'a' && c <= 'f':
		return c - 'a' + 10, true
	case c >= 'A' && c <= 'F':
		return c - 'A' + 10, true
	default:
		return 0, false
	}
}
