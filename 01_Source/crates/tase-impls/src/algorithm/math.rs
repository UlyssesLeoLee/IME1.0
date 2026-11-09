//! The six number algorithms of the catalogue (the assets `algorithm.math.*`).
//!
//! Every public function of this module names its asset in its doc comment:
//! `* asset: `algorithm.\<category\>.\<name\>``.
//!
//! | function | asset | what it does |
//! |---|---|---|
//! | [`euclidean_gcd`] | `algorithm.math.euclidean_gcd` | the greatest common divisor of two integers |
//! | [`sieve_of_eratosthenes`] | `algorithm.math.sieve_of_eratosthenes` | every prime up to a limit |
//! | [`fast_power`] | `algorithm.math.fast_power` | an exact integer power, `O(log e)` multiplications |
//! | [`modular_exponentiation`] | `algorithm.math.modular_exponentiation` | an integer power taken modulo `m` |
//! | [`extended_euclidean`] | `algorithm.math.extended_euclidean` | the greatest common divisor and its coefficients |
//! | [`reservoir_sampling`] | `algorithm.math.reservoir_sampling` | `k` uniform samples of a stream of unknown length |
//!
//! # The rules of this module
//!
//! * **Deterministic.** The same input always gives the same output. The one function that needs randomness,
//!   [`reservoir_sampling`], takes the source of the random numbers as an argument, so the caller decides how the
//!   numbers are drawn and can make the run reproducible with a fixed seed.
//! * **No panic on a valid input.** The impossible cases are reported as an `Err` with a readable reason: a
//!   modulus of zero, an exponent whose result does not fit into the type, a limit that does not fit into memory,
//!   the degenerate pair `(0, 0)` and a random source that returns an index outside its range.
//! * **The standard library only.** The crate has no dependency at all, and every dependency would need a licence
//!   review (design §1.2).

use std::error::Error;
use std::fmt;

/// The largest limit [`sieve_of_eratosthenes`] accepts.
///
/// The sieve keeps one flag per number, so the limit has to fit into memory: 100 000 000 needs about 12 MiB of
/// flags. The limit above it is reported as [`MathError::LimitTooLarge`]; the card then points at a segmented sieve
/// or at another method.
pub const MAX_SIEVE_LIMIT: usize = 100_000_000;

/// The failures the number assets of this module can report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MathError {
    /// The modulus of a modular operation is zero, so the result is not defined.
    ZeroModulus,
    /// The result of the operation does not fit into the type of the result.
    Overflow {
        /// What was being computed, in a few words.
        what: &'static str,
    },
    /// The pair `(0, 0)` has no meaningful coefficients.
    BothZero,
    /// The limit is beyond what the memory can hold.
    LimitTooLarge {
        /// The limit that was asked for.
        limit: usize,
        /// The largest limit the function accepts.
        max: usize,
    },
    /// The source of the random numbers returned an index outside the range it was given.
    RandomIndexOutOfRange {
        /// The index the source returned.
        index: usize,
        /// The exclusive upper bound the source was given.
        bound: usize,
    },
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MathError::ZeroModulus => write!(f, "the modulus is zero, the result is not defined"),
            MathError::Overflow { what } => write!(f, "the arithmetic overflowed while computing {what}"),
            MathError::BothZero => {
                write!(f, "the pair (0, 0) has no Bézout coefficients: every pair of coefficients works")
            }
            MathError::LimitTooLarge { limit, max } => write!(
                f,
                "the limit {limit} is beyond the limit {max} of the sieve: the flags would not fit into memory"
            ),
            MathError::RandomIndexOutOfRange { index, bound } => write!(
                f,
                "the source of the random numbers returned the index {index}, which is outside the range \
                 0..{bound}"
            ),
        }
    }
}

impl Error for MathError {}

/// Euclidean algorithm: the greatest common divisor of two non-negative integers.
///
/// The larger of the two numbers is replaced by the remainder of the division, and the numbers shrink with every
/// step, so the loop always ends.
///
/// * asset: `algorithm.math.euclidean_gcd`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.math.euclidean_gcd/card.md`
///
/// # Input preconditions (from the card)
///
/// * `input: nonnegative_integers` — the numbers are `u64`, so a negative input is not representable and cannot
///   reach the function. Take the absolute value before the call.
/// * `overflow: remainder_stays_bounded` — the algorithm only uses the remainder, so no value can grow.
/// * `operation: gcd` — the function answers with the greatest common divisor and nothing else.
///
/// # Complexity (from the card)
///
/// * Time `O(log min(a, b))`; the worst case is a pair of neighbouring Fibonacci numbers.
/// * Extra space `O(1)`: the loop keeps no stack.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The coefficients of the Bézout identity are needed*: "需要贝祖系数 s, t 使 sa + tb = gcd(a, b)——用扩展欧几里得
///   算法" — use [`extended_euclidean`].
/// * *The input is negative*: "输入含负数——需先取绝对值（或用带符号的扩展版本）" — take the absolute value first.
/// * *The input is a float or a rational*: "输入是浮点数或有理数——本算法只对整数成立".
///
/// # Determinism
///
/// The result depends only on the two numbers. `gcd(0, 0)` is `0`, `gcd(0, n)` is `n` and `gcd(n, n)` is `n`.
pub fn euclidean_gcd(a: u64, b: u64) -> u64 {
    let mut x = a;
    let mut y = b;
    while y != 0 {
        let remainder = x % y;
        x = y;
        y = remainder;
    }
    x
}

/// Sieve of Eratosthenes: every prime number up to a limit.
///
/// The numbers from 2 up to the square root of the limit are walked through, and every multiple of a prime is
/// marked as composite. The numbers that are still unmarked are the primes.
///
/// * asset: `algorithm.math.sieve_of_eratosthenes`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.math.sieve_of_eratosthenes/card.md`
///
/// # Input preconditions (from the card)
///
/// * `output: all_primes_up_to_n` — the result holds every prime up to and including the limit, in ascending order.
/// * `bound: n_in_memory` — the flags of the sieve have to fit into memory, so the limit may not be larger than
///   [`MAX_SIEVE_LIMIT`]. A larger limit is reported as [`MathError::LimitTooLarge`].
/// * `marking: from_p_squared` — the multiples are marked from `p * p`, which is what keeps the sieve at
///   `O(n log log n)`.
///
/// # Complexity (from the card)
///
/// * Time `O(n log log n)`.
/// * Extra space `O(n)`: one flag per number. The flags of a `Vec<bool>` are stored as bits, so `n` needs `n / 8`
///   bytes.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *Only one big number has to be tested*: "只需要判断一个大数是否为素数时——用确定性 Miller-Rabin 更省" — the
///   sieve builds the whole table for one question.
/// * *The limit is enormous*: "n 极大（如 10^12）时——O(n) 空间不可接受，需用分段筛或其它方法" — the function
///   reports [`MathError::LimitTooLarge`].
/// * *The factors are needed, not the primality*: "需要因子分解而非素性判断——用试除或 Pollard Rho（注意：这是密码学
///   相关，应使用成熟库）".
/// * *The limit is small*: "n 很小——试除即可".
///
/// # Determinism
///
/// The primes are a property of the limit, so the result is unique.
pub fn sieve_of_eratosthenes(limit: usize) -> Result<Vec<usize>, MathError> {
    if limit > MAX_SIEVE_LIMIT {
        return Err(MathError::LimitTooLarge {
            limit,
            max: MAX_SIEVE_LIMIT,
        });
    }
    if limit < 2 {
        return Ok(Vec::new());
    }
    let mut is_prime = vec![true; limit + 1];
    is_prime[0] = false;
    is_prime[1] = false;
    let mut p = 2usize;
    // `p <= limit / p` is `p * p <= limit` without the overflow of the multiplication.
    while p <= limit / p {
        if is_prime[p] {
            let mut multiple = p * p;
            while multiple <= limit {
                is_prime[multiple] = false;
                multiple += p;
            }
        }
        p += 1;
    }
    let mut primes = Vec::new();
    for (number, &flag) in is_prime.iter().enumerate() {
        if flag {
            primes.push(number);
        }
    }
    Ok(primes)
}

/// Fast power: an exact integer power, computed with squaring.
///
/// The exponent is written in binary: the base is squared once per bit of the exponent, and the result is
/// multiplied by the base whenever the bit is set.
///
/// * asset: `algorithm.math.fast_power`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.math.fast_power/card.md`
///
/// # Input preconditions (from the card)
///
/// * `operation: base^exponent` — the base is an `i64` and the exponent a `u64`.
/// * `overflow: must_be_checked` — the result has to fit into an `i64`. When it does not, the function reports
///   [`MathError::Overflow`] instead of wrapping around; this is the trap the card names ("中间值 b = b*b 极易溢出").
/// * `modulus: optional` — without a modulus the exact value is computed; when the value does not fit, use
///   [`modular_exponentiation`].
///
/// # Complexity (from the card)
///
/// * Time `O(log exponent)` multiplications.
/// * Extra space `O(1)`.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The exponent is small*: "指数很小——朴素连乘更简单".
/// * *The exact result is needed and does not fit*: "结果需要精确且会溢出——必须改用模幂或大整数" — the function
///   reports [`MathError::Overflow`]; use [`modular_exponentiation`] or a big integer library.
/// * *The base is a float*: "浮点数场景——直接用 `powf` 即可，快速幂是整数算法".
///
/// # Determinism
///
/// The result depends only on the two numbers. The conventions of the extreme cases are written down: an exponent
/// of zero gives `1`, and that also holds for the base `0` (`0^0` is `1`, the value of the empty product).
pub fn fast_power(base: i64, exponent: u64) -> Result<i64, MathError> {
    if exponent == 0 {
        // The empty product, also for the base 0: 0^0 is 1.
        return Ok(1);
    }
    let mut result: i64 = 1;
    let mut factor = base;
    let mut rest = exponent;
    while rest > 0 {
        if rest & 1 == 1 {
            result = result
                .checked_mul(factor)
                .ok_or(MathError::Overflow { what: "base^exponent" })?;
        }
        rest >>= 1;
        if rest > 0 {
            // The last square is not needed and would report an overflow that does not happen.
            factor = factor
                .checked_mul(factor)
                .ok_or(MathError::Overflow { what: "base^exponent" })?;
        }
    }
    Ok(result)
}

/// Modular exponentiation: an integer power taken modulo a modulus.
///
/// The same squaring as in [`fast_power`], but every multiplication is followed by a reduction modulo the
/// modulus. The intermediate product is computed in a `u128`, so it cannot overflow even when the modulus is
/// close to the largest `u64`.
///
/// * asset: `algorithm.math.modular_exponentiation`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.math.modular_exponentiation/card.md`
///
/// # Input preconditions (from the card)
///
/// * `modulus: positive` — the modulus has to be greater than zero. A modulus of zero is reported as
///   [`MathError::ZeroModulus`], the card asks for an error and not a panic.
/// * `overflow: intermediate_product_can_exceed_type` — the product of two residues does not fit into a `u64`,
///   so it is computed in a `u128`; this is the bug the card names ("`mul_mod` 里 `a * b` 在 u64 下会溢出").
/// * The base is reduced modulo the modulus before the loop, so a base that is larger than the modulus is fine.
///
/// # Complexity (from the card)
///
/// * Time `O(log exponent)` modular multiplications.
/// * Extra space `O(1)`.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *No modulus is needed and the exponent is small*: "不需要取模且指数小——直接乘法即可".
/// * *The modulus is a prime and the inverse is needed*: "模数不是质数且需要费马小定理化简——该定理要求质数，误用会
///   得到错误结果" — use [`extended_euclidean`] for the inverse.
/// * *The modulus is larger than 128 bits*: "模数极大（超过 128 位）——需要大整数库".
/// * *The use is cryptographic*: "密码学场景下的实现必须使用经过审查的库" — this implementation is not audited.
///
/// # Determinism
///
/// The result depends only on the three numbers. The conventions of the extreme cases are written down: an
/// exponent of zero gives `1 % m` and a modulus of one gives `0`.
pub fn modular_exponentiation(
    base: u64,
    exponent: u64,
    modulus: u64,
) -> Result<u64, MathError> {
    if modulus == 0 {
        return Err(MathError::ZeroModulus);
    }
    // The residues and their product are computed in a u128: the product of two u64 does not fit into a u64.
    let m = u128::from(modulus);
    let mut result: u128 = 1 % m;
    let mut factor: u128 = u128::from(base) % m;
    let mut rest = exponent;
    while rest > 0 {
        if rest & 1 == 1 {
            result = (result * factor) % m;
        }
        rest >>= 1;
        if rest > 0 {
            factor = (factor * factor) % m;
        }
    }
    Ok(result as u64)
}

/// Extended Euclidean algorithm: the greatest common divisor and the coefficients of the Bézout identity.
///
/// The division steps of the Euclidean algorithm are repeated, and the coefficients of the two numbers are carried
/// along. The result is the triple `(g, s, t)` with `s * a + t * b == g`.
///
/// * asset: `algorithm.math.extended_euclidean`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.math.extended_euclidean/card.md`
///
/// # Input preconditions (from the card)
///
/// * `input: integers_any_sign` — the two numbers are `i128` and may be negative; the sign is handled by the
///   algorithm.
/// * `output: gcd_and_coefficients` — the result is the greatest common divisor and the two coefficients.
/// * The pair `(0, 0)` is reported as [`MathError::BothZero`]: the identity holds for every pair of coefficients,
///   so there is nothing to report.
/// * The coefficients may be larger than the input; they are computed in an `i128`. When even that is too narrow,
///   the function reports [`MathError::Overflow`].
///
/// # Complexity (from the card)
///
/// * Time `O(log min(a, b))`.
/// * Extra space `O(1)`: the implementation is iterative, so there is no stack depth to worry about.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *Only the greatest common divisor is needed*: "只需要最大公约数时——普通欧几里得算法更简单" — use
///   [`euclidean_gcd`].
/// * *The modulus is a prime and the inverse is needed*: "模数是质数时——费马小定理给逆元更快，但要求互质" — with a
///   prime modulus `a^(m-2) mod m` is faster, see [`modular_exponentiation`].
/// * *Only the coprimality has to be checked*: "只需要判断是否互质——普通 gcd 足够".
/// * *The input is a float*: "输入是浮点数——本算法只对整数成立".
///
/// # The modular inverse
///
/// The inverse of `a` modulo `m` is the coefficient `s` of this function, taken modulo `m`, and it exists only when
/// the greatest common divisor is `1`:
///
/// ```
/// let (g, s, _) = tase_impls::algorithm::math::extended_euclidean(3, 11).expect("a pair of integers");
/// assert_eq!(g, 1);
/// assert_eq!(s.rem_euclid(11), 4, "3 * 4 is 1 modulo 11");
/// ```
///
/// # Determinism
///
/// The result depends only on the two numbers. Between the many pairs of coefficients that satisfy the identity
/// the implementation returns the one of the standard division steps, which is the one the card describes.
pub fn extended_euclidean(a: i128, b: i128) -> Result<(i128, i128, i128), MathError> {
    if a == 0 && b == 0 {
        return Err(MathError::BothZero);
    }
    let mut old_r = a;
    let mut r = b;
    let mut old_s: i128 = 1;
    let mut s: i128 = 0;
    let mut old_t: i128 = 0;
    let mut t: i128 = 1;
    while r != 0 {
        // `i128::MIN / -1` would panic, so the division is checked.
        let quotient = old_r
            .checked_div(r)
            .ok_or(MathError::Overflow { what: "the quotient" })?;
        let next_r = old_r
            .checked_sub(quotient.checked_mul(r).ok_or(MathError::Overflow {
                what: "the remainder",
            })?)
            .ok_or(MathError::Overflow { what: "the remainder" })?;
        old_r = r;
        r = next_r;
        let next_s = old_s
            .checked_sub(
                quotient
                    .checked_mul(s)
                    .ok_or(MathError::Overflow { what: "the coefficients" })?,
            )
            .ok_or(MathError::Overflow { what: "the coefficients" })?;
        old_s = s;
        s = next_s;
        let next_t = old_t
            .checked_sub(
                quotient
                    .checked_mul(t)
                    .ok_or(MathError::Overflow { what: "the coefficients" })?,
            )
            .ok_or(MathError::Overflow { what: "the coefficients" })?;
        old_t = t;
        t = next_t;
    }
    Ok((old_r, old_s, old_t))
}

/// Reservoir sampling: `k` uniform samples of a stream whose length is not known in advance.
///
/// The first `k` elements fill the reservoir. Every later element has the chance `k / i` to enter it, and the
/// element it replaces is drawn at random as well, so every element of the stream has the same chance of being in
/// the sample.
///
/// * asset: `algorithm.math.reservoir_sampling`
/// * card: `01_Source/assets/tase-catalog/assets/algorithm.math.reservoir_sampling/card.md`
///
/// # Input preconditions (from the card)
///
/// * `stream_length: unknown_in_advance` — the input is any iterator and is read once.
/// * `passes: 1` — the stream is read exactly once.
/// * `uniformity: exact` — the source of the random numbers has to be uniform. It is called with an exclusive
///   upper bound and has to return a value in `0..bound`; a value outside that range is reported as
///   [`MathError::RandomIndexOutOfRange`], which is the "确定的退化策略" the card asks for.
/// * `space: O(k)` — the reservoir is the only memory the function needs.
///
/// # Complexity (from the card)
///
/// * Time `O(n)` in total: one random number and a few operations per element of the stream.
/// * Extra space `O(k)`: the reservoir, and nothing that grows with the length of the stream.
///
/// # When it does not apply (from the card's counterexamples)
///
/// * *The length of the stream is known*: "已知总数——直接随机选下标更简单".
/// * *Two passes over the data are possible*: "可以两遍扫描——第一遍计数，第二遍按概率抽样".
/// * *The sample has to be weighted*: "需要加权抽样——等概率蓄水池不适用".
/// * *The sample has to be taken per segment*: "需要按时间分段抽样——应使用分层抽样".
///
/// # Determinism
///
/// The sample itself is random, but the run is reproducible: the source of the random numbers is an argument, so
/// a caller with a fixed seed gets the same sample every time. The extreme cases are written down: a `k` of zero
/// gives an empty sample and does not read the stream, and a stream that is shorter than `k` gives all of its
/// elements.
pub fn reservoir_sampling<T>(
    items: impl IntoIterator<Item = T>,
    k: usize,
    mut sample_index: impl FnMut(usize) -> usize,
) -> Result<Vec<T>, MathError> {
    if k == 0 {
        // An empty reservoir: the stream is not read at all.
        return Ok(Vec::new());
    }
    // No `with_capacity`: an enormous k must not turn into a failed allocation.
    let mut reservoir: Vec<T> = Vec::new();
    let mut seen: usize = 0;
    for item in items {
        seen += 1;
        if reservoir.len() < k {
            reservoir.push(item);
            continue;
        }
        // The element that arrives has the chance k / seen to enter the reservoir.
        let index = sample_index(seen);
        if index >= seen {
            return Err(MathError::RandomIndexOutOfRange {
                index,
                bound: seen,
            });
        }
        if index < k {
            reservoir[index] = item;
        }
    }
    Ok(reservoir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A linear congruential generator with a fixed seed, so that the tests never depend on a real random
    /// number generator. The modulo of [`Lcg::below`] has a small bias, which does not matter for a test.
    struct Lcg(u64);

    impl Lcg {
        /// A generator with the given seed.
        fn new(seed: u64) -> Self {
            Lcg(seed)
        }

        /// The next value of the sequence.
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 11
        }

        /// A value in `0..bound`, taken from the sequence.
        fn below(&mut self, bound: usize) -> usize {
            assert!(bound > 0, "the bound of the random source is never zero");
            (self.next() % bound as u64) as usize
        }
    }

    #[test]
    fn euclidean_gcd_computes_the_greatest_common_divisor() {
        assert_eq!(euclidean_gcd(12, 18), 6);
        assert_eq!(euclidean_gcd(18, 12), 6, "the order of the arguments does not matter");
        assert_eq!(euclidean_gcd(17, 5), 1, "a coprime pair gives one");
        assert_eq!(euclidean_gcd(0, 0), 0, "the boundary of the card");
        assert_eq!(euclidean_gcd(0, 9), 9);
        assert_eq!(euclidean_gcd(9, 0), 9);
        assert_eq!(euclidean_gcd(7, 7), 7);
        assert_eq!(euclidean_gcd(u64::MAX, u64::MAX - 1), 1);

        // The worst case of the card: two neighbouring Fibonacci numbers.
        assert_eq!(euclidean_gcd(8_675_309, 5_702_887), 1);
    }

    #[test]
    fn euclidean_gcd_does_not_give_the_coefficients_of_the_bezout_identity() {
        // The counterexample of the card: the coefficients need the extended algorithm.
        assert_eq!(euclidean_gcd(240, 46), 2);
        let (g, s, t) = extended_euclidean(240, 46).expect("a pair of integers");
        assert_eq!(g, 2);
        assert_eq!(s * 240 + t * 46, 2, "the extended algorithm adds the coefficients");
    }

    #[test]
    fn sieve_of_eratosthenes_lists_the_primes_up_to_the_limit() {
        assert_eq!(sieve_of_eratosthenes(0).expect("a limit of zero is valid"), Vec::<usize>::new());
        assert_eq!(sieve_of_eratosthenes(1).expect("a limit of one is valid"), Vec::<usize>::new());
        assert_eq!(sieve_of_eratosthenes(2).expect("a limit of two is valid"), vec![2]);
        assert_eq!(
            sieve_of_eratosthenes(20).expect("the limit is small enough"),
            vec![2, 3, 5, 7, 11, 13, 17, 19]
        );
        assert_eq!(
            sieve_of_eratosthenes(100).expect("the limit is small enough"),
            vec![2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97]
        );

        // The property of the card: every prime up to the limit is in the result.
        let primes = sieve_of_eratosthenes(500).expect("the limit is small enough");
        for number in 2..=500usize {
            let is_prime = (2..number).all(|divisor| number % divisor != 0);
            assert_eq!(primes.contains(&number), is_prime, "{number} is judged wrong");
        }
    }

    #[test]
    fn sieve_of_eratosthenes_rejects_a_limit_beyond_the_memory() {
        // The counterexample of the card: a limit of 10^12 does not fit into memory.
        let error = sieve_of_eratosthenes(MAX_SIEVE_LIMIT + 1).expect_err("the limit is too large");
        assert_eq!(
            error,
            MathError::LimitTooLarge {
                limit: MAX_SIEVE_LIMIT + 1,
                max: MAX_SIEVE_LIMIT,
            }
        );
    }

    #[test]
    fn fast_power_computes_the_exact_power() {
        assert_eq!(fast_power(2, 0), Ok(1));
        assert_eq!(fast_power(2, 1), Ok(2));
        assert_eq!(fast_power(2, 10), Ok(1024));
        assert_eq!(fast_power(3, 5), Ok(243));
        assert_eq!(fast_power(0, 0), Ok(1), "the convention of the card: 0^0 is the empty product");
        assert_eq!(fast_power(0, 4), Ok(0));
        assert_eq!(fast_power(1, 1000), Ok(1));
        assert_eq!(fast_power(-2, 3), Ok(-8));
        assert_eq!(fast_power(-2, 4), Ok(16));
        assert_eq!(fast_power(-1, 7), Ok(-1));

        // The property of the card: the power of two is the shift.
        for exponent in 0..32u64 {
            assert_eq!(fast_power(2, exponent), Ok(1i64 << exponent));
        }
    }

    #[test]
    fn fast_power_reports_an_overflow_instead_of_wrapping_around() {
        // The trap of the card: the intermediate square overflows.
        let error = fast_power(2, 63).expect_err("the result does not fit into an i64");
        assert_eq!(error, MathError::Overflow { what: "base^exponent" });
        assert_eq!(fast_power(i64::MIN, 2), Err(MathError::Overflow { what: "base^exponent" }));
    }

    #[test]
    fn modular_exponentiation_computes_the_power_modulo_the_modulus() {
        assert_eq!(modular_exponentiation(2, 10, 1_000_000_007), Ok(1024));
        assert_eq!(modular_exponentiation(123_456_789, 1_000_000_000, 1_000_000_007), Ok(161_045_046));
        // The extreme cases of the card.
        assert_eq!(modular_exponentiation(5, 0, 7), Ok(1), "an exponent of zero gives 1 modulo m");
        assert_eq!(modular_exponentiation(0, 5, 7), Ok(0));
        assert_eq!(modular_exponentiation(5, 3, 1), Ok(0), "a modulus of one gives zero");
        assert_eq!(modular_exponentiation(8, 1, 3), Ok(2), "the base is reduced modulo m");

        // The modulus close to the largest u64: the product of two residues does not fit into a u64.
        assert_eq!(
            modular_exponentiation(u64::MAX, 2, u64::MAX),
            Ok(0),
            "the base is reduced modulo m first"
        );
        assert_eq!(
            modular_exponentiation(1u64 << 63, 2, u64::MAX),
            Ok(4_611_686_018_427_387_904),
            "the product of two residues is computed in a u128"
        );
        assert_eq!(modular_exponentiation(2, u64::MAX, 1_000_000_007), Ok(981_530_768));
    }

    #[test]
    fn modular_exponentiation_rejects_a_modulus_of_zero() {
        // The card asks for an error and not a panic.
        assert_eq!(modular_exponentiation(2, 10, 0), Err(MathError::ZeroModulus));
    }

    #[test]
    fn extended_euclidean_returns_the_bezout_coefficients() {
        let (g, s, t) = extended_euclidean(240, 46).expect("a pair of integers");
        assert_eq!(g, 2);
        assert_eq!(s * 240 + t * 46, 2, "the identity of the card");

        // The boundary cases of the card.
        let (g, s, t) = extended_euclidean(5, 0).expect("a pair of integers");
        assert_eq!((g, s, t), (5, 1, 0));
        let (g, s, t) = extended_euclidean(0, 5).expect("a pair of integers");
        assert_eq!((g, s, t), (5, 0, 1));
        let (g, s, t) = extended_euclidean(7, 7).expect("a pair of integers");
        assert_eq!(g, 7);
        assert_eq!(s * 7 + t * 7, 7);

        // The sign of the input is handled.
        let (g, s, t) = extended_euclidean(-12, 18).expect("a pair of integers");
        assert_eq!(g, 6);
        assert_eq!(s * -12 + t * 18, 6);

        // The modular inverse of the card.
        let (g, s, _) = extended_euclidean(3, 11).expect("a pair of integers");
        assert_eq!(g, 1);
        assert_eq!(s.rem_euclid(11) * 3 % 11, 1);
    }

    #[test]
    fn extended_euclidean_rejects_the_degenerate_pair_of_zeros() {
        // Every pair of coefficients satisfies the identity for (0, 0), so there is nothing to report.
        assert_eq!(extended_euclidean(0, 0), Err(MathError::BothZero));
    }

    #[test]
    fn reservoir_sampling_returns_k_distinct_elements_of_the_stream() {
        let mut rng = Lcg::new(20_260_907);
        let sample = reservoir_sampling(0..100i32, 10, |bound| rng.below(bound))
            .expect("the source stays inside its range");
        assert_eq!(sample.len(), 10);
        let mut sorted = sample.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 10, "the sample holds ten different elements");
        assert!(sample.iter().all(|value| (0..100).contains(value)));

        // A stream that is shorter than k gives all of its elements.
        let mut rng = Lcg::new(7);
        let sample = reservoir_sampling(0..3i32, 10, |bound| rng.below(bound)).expect("the source stays inside its range");
        assert_eq!(sample, vec![0, 1, 2]);

        // The empty stream gives an empty sample.
        let mut rng = Lcg::new(7);
        let sample = reservoir_sampling(Vec::<i32>::new(), 3, |bound| rng.below(bound)).expect("the source stays inside its range");
        assert!(sample.is_empty());
    }

    #[test]
    fn reservoir_sampling_spreads_the_sample_over_the_whole_stream() {
        // The property of the card: the elements are drawn with the same chance.
        let mut rng = Lcg::new(4_242);
        let mut counts = [0usize; 20];
        for _ in 0..400 {
            let sample = reservoir_sampling(0..20i32, 2, |bound| rng.below(bound)).expect("the source stays inside its range");
            for value in sample {
                counts[value as usize] += 1;
            }
        }
        // 800 draws over 20 elements: on average 40 each. The bounds are wide enough to hold the wobble of a
        // fixed seed but narrow enough to catch a reservoir that is not uniform.
        assert!(counts.iter().all(|count| *count > 10), "some element is drawn too rarely: {counts:?}");
        assert!(counts.iter().all(|count| *count < 80), "some element is drawn too often: {counts:?}");
        assert_eq!(counts.iter().sum::<usize>(), 800);
    }

    #[test]
    fn reservoir_sampling_rejects_a_source_that_leaves_its_range() {
        // The card asks for a fixed answer when the source of the random numbers fails. The reservoir is full
        // after the first two elements, so the source is asked for the third one and returns its bound.
        let error = reservoir_sampling(0..10i32, 2, |bound| bound).expect_err("the source leaves its range");
        assert_eq!(error, MathError::RandomIndexOutOfRange { index: 3, bound: 3 });
    }
}
