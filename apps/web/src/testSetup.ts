class NoopResizeObserver implements ResizeObserver {
  constructor(callback: ResizeObserverCallback) {
    void callback;
  }

  disconnect(): void {}

  observe(): void {}

  unobserve(): void {}
}

window.ResizeObserver = NoopResizeObserver;
