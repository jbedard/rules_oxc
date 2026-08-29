function dec(_target: unknown, _key?: string): void {}

export class Service {
  @dec
  name: string = "svc";

  @dec
  describe(label: string | null): number {
    return label ? label.length : 0;
  }
}
