export function Pair(props: { a: string; b: string }): unknown {
  return (
    <>
      <span>{props.a}</span>
      <span>{props.b}</span>
    </>
  );
}
