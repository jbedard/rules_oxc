export const big: number = 2 ** 10;

export async function fetchValue(): Promise<number> {
  return big;
}

export function pick(input: { nested?: { value?: number } }): number {
  return input.nested?.value ?? 0;
}
