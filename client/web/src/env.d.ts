interface WebSocket {
    send(data: string | ArrayBufferLike | Blob | ArrayBufferView<ArrayBufferLike>): void;
}
