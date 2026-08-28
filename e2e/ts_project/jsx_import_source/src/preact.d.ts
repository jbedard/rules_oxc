declare module "preact/jsx-runtime" {
  export namespace JSX {
    interface IntrinsicElements {
      [name: string]: unknown;
    }
  }
}
