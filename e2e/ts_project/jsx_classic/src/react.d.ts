declare const React: {
  createElement(type: unknown, props: unknown, ...children: unknown[]): unknown;
};

declare namespace JSX {
  interface IntrinsicElements {
    [name: string]: unknown;
  }
}
