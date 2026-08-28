declare function h(type: unknown, props: unknown, ...children: unknown[]): unknown;
declare function Fragment(props: { children?: unknown }): unknown;

declare namespace JSX {
  interface IntrinsicElements {
    [name: string]: unknown;
  }
}
