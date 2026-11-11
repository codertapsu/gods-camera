package dev.godscamera.android

/** How many digits the Mac's code has. */
const val CODE_LENGTH = 6

/**
 * What the code field holds after a person typed or pasted [typed]: digits
 * only, six at most. Spaces and dashes are dropped, since the Mac shows the
 * code as "123 456".
 */
fun codeDigits(typed: String): String = typed.filter { it in '0'..'9' }.take(CODE_LENGTH)

fun isCompleteCode(digits: String): Boolean = digits.length == CODE_LENGTH && digits.all { it in '0'..'9' }
