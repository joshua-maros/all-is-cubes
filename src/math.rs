// Copyright 2020 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <http://opensource.org/licenses/MIT>.

//! Mathematical utilities and decisions.

use std::ops::{Rem, Add};
use cgmath::{BaseNum, EuclideanSpace, Point3, Vector3};

pub type GridCoordinate = isize;
pub type GridPoint = Point3<GridCoordinate>;
pub type FreeCoordinate = f64;

pub trait Modulo<M = Self> {
    type Output;
    
    fn modulo(self, modulus :M) -> Self::Output;
}

// Implementing Modulo on a case-by-case basis because the compiler objected
// to providing impls for both Vector3 and the full generality of modulo_impl.
impl Modulo for f32 {
    type Output = Self;
    fn modulo(self, modulus :Self) -> Self { modulo_impl(self, modulus) }
}
impl Modulo for f64 {
    type Output = Self;
    fn modulo(self, modulus :Self) -> Self { modulo_impl(self, modulus) }
}
impl<S : Modulo<S, Output = S> + Copy> Modulo<S> for Vector3<S> {
    type Output = Self;
    fn modulo(self, modulus :S) -> Self { self.map(|x| x.modulo(modulus)) }
}
impl<S : BaseNum + Modulo<S, Output = S>> Modulo<S> for Point3<S> {
    type Output = Vector3<S>;
    fn modulo(self, modulus :S) -> Vector3<S> { self.to_vec().modulo(modulus) }
}

/// Implement modulo in terms of remainder and addition.
fn modulo_impl<
    T: Rem<M, Output = T> + Add<M, Output = T>,
    M: Copy,
>(value :T, modulus :M) -> T {
    // Remainder, which lies in the range (-modulus, +modulus).
    let remainder :T = value % modulus;
    // Shift the range to (0, 2*modulus).
    let guaranteed_positive :T = remainder + modulus;
    // Collapse the two cases (0, modulus) and [modulus, 2*modulus) to [0, modulus).
    return guaranteed_positive % modulus;
}


#[cfg(test)]
mod tests {
    use super::*;
    use cgmath::Vector3;

    // Tests for modulo, which is not currently a public function so can't have doc tests.

    #[test]
    fn modulo_positive() { 
        assert_eq!(0.0.modulo(1.0), 0.0);
        assert_eq!(0.25.modulo(1.0), 0.25);
        assert_eq!(1.0.modulo(1.0), 0.0);
        assert_eq!(1.25.modulo(1.0), 0.25);
        assert_eq!(6.25.modulo(1.0), 0.25);
        
        assert_eq!(0.0.modulo(1.5), 0.0);
        assert_eq!(1.0.modulo(1.5), 1.0);
        assert_eq!(1.5.modulo(1.5), 0.0);
        assert_eq!(1.625.modulo(1.5), 0.125);
    }

    #[test]
    fn modulo_negative_value() { 
        assert_eq!((-0.0).modulo(1.0), 0.0);
        assert_eq!((-0.25).modulo(1.0), 0.75);
        assert_eq!((-1.0).modulo(1.0), 0.0);
        assert_eq!((-1.25).modulo(1.0), 0.75);
        assert_eq!((-6.25).modulo(1.0), 0.75);
    }

    #[test]
    fn modulo_negative_modulus() { 
        assert_eq!(0.0.modulo(-1.0), -0.0);
        assert_eq!(0.25.modulo(-1.0), -0.75);
        assert_eq!(1.0.modulo(-1.0), -0.0);
        assert_eq!(1.25.modulo(-1.0), -0.75);
        assert_eq!(6.25.modulo(-1.0), -0.75);
    }
    
    #[test]
    fn modulo_of_vector() {
        assert_eq!(
            Vector3::new(1.25 as f64, 2.75, -3.25).modulo(1.0),
            Vector3::new(0.25, 0.75, 0.75));
    }

    #[test]
    // Note: Not specifically desiring this behavior, just documenting it.
    fn modulo_zero_float() { assert!((3.0 as f64).modulo(0.0).is_nan()); }
}