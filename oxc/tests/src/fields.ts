export class Fields {
  declared: number;
  initialized: number = 1;
  static count: number;
  static label: string = "fields";

  constructor(value: number) {
    this.declared = value;
  }
}
