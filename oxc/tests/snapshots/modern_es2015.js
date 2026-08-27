import _asyncToGenerator from "@oxc-project/runtime/helpers/asyncToGenerator";
export const big = Math.pow(2, 10);
export function fetchValue() {
	return _fetchValue.apply(this, arguments);
}
function _fetchValue() {
	_fetchValue = _asyncToGenerator(function* () {
		return big;
	});
	return _fetchValue.apply(this, arguments);
}
export function pick(input) {
	var _input$nested$value, _input$nested;
	return (_input$nested$value = (_input$nested = input.nested) === null || _input$nested === void 0 ? void 0 : _input$nested.value) !== null && _input$nested$value !== void 0 ? _input$nested$value : 0;
}
