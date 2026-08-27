export const big = 2 ** 10;
export async function fetchValue() {
	return big;
}
export function pick(input) {
	var _input$nested$value, _input$nested;
	return (_input$nested$value = (_input$nested = input.nested) === null || _input$nested === void 0 ? void 0 : _input$nested.value) !== null && _input$nested$value !== void 0 ? _input$nested$value : 0;
}
