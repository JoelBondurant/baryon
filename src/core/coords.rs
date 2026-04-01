use std::fmt;

macro_rules! impl_u32_unit {
	($name:ident) => {
		#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
		pub struct $name(u32);

		impl $name {
			pub const ZERO: Self = Self(0);

			pub const fn new(value: u32) -> Self {
				Self(value)
			}

			pub const fn get(self) -> u32 {
				self.0
			}

			pub const fn saturating_add(self, rhs: u32) -> Self {
				Self(self.0.saturating_add(rhs))
			}

			pub const fn saturating_sub(self, rhs: u32) -> Self {
				Self(self.0.saturating_sub(rhs))
			}
		}

		impl fmt::Display for $name {
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				self.0.fmt(f)
			}
		}

		impl std::ops::Add<u32> for $name {
			type Output = Self;

			fn add(self, rhs: u32) -> Self::Output {
				Self(self.0 + rhs)
			}
		}

		impl std::ops::AddAssign<u32> for $name {
			fn add_assign(&mut self, rhs: u32) {
				self.0 += rhs;
			}
		}

		impl std::ops::Sub<u32> for $name {
			type Output = Self;

			fn sub(self, rhs: u32) -> Self::Output {
				Self(self.0.saturating_sub(rhs))
			}
		}

		impl std::ops::SubAssign<u32> for $name {
			fn sub_assign(&mut self, rhs: u32) {
				self.0 = self.0.saturating_sub(rhs);
			}
		}

		impl From<u32> for $name {
			fn from(value: u32) -> Self {
				Self(value)
			}
		}

		impl From<$name> for u32 {
			fn from(value: $name) -> Self {
				value.0
			}
		}
	};
}

macro_rules! impl_u64_unit {
	($name:ident) => {
		#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
		pub struct $name(u64);

		impl $name {
			pub const ZERO: Self = Self(0);

			pub const fn new(value: u64) -> Self {
				Self(value)
			}

			pub const fn get(self) -> u64 {
				self.0
			}

			pub const fn saturating_add(self, rhs: u64) -> Self {
				Self(self.0.saturating_add(rhs))
			}

			pub const fn saturating_sub(self, rhs: u64) -> Self {
				Self(self.0.saturating_sub(rhs))
			}
		}

		impl fmt::Display for $name {
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				self.0.fmt(f)
			}
		}

		impl std::ops::Add<u64> for $name {
			type Output = Self;

			fn add(self, rhs: u64) -> Self::Output {
				Self(self.0 + rhs)
			}
		}

		impl std::ops::AddAssign<u64> for $name {
			fn add_assign(&mut self, rhs: u64) {
				self.0 += rhs;
			}
		}

		impl std::ops::Sub<u64> for $name {
			type Output = Self;

			fn sub(self, rhs: u64) -> Self::Output {
				Self(self.0.saturating_sub(rhs))
			}
		}

		impl std::ops::SubAssign<u64> for $name {
			fn sub_assign(&mut self, rhs: u64) {
				self.0 = self.0.saturating_sub(rhs);
			}
		}

		impl From<u64> for $name {
			fn from(value: u64) -> Self {
				Self(value)
			}
		}

		impl From<$name> for u64 {
			fn from(value: $name) -> Self {
				value.0
			}
		}
	};
}

impl_u32_unit!(DocLine);
impl_u32_unit!(VisualCol);
impl_u32_unit!(NodeByteOffset);
impl_u32_unit!(ScreenRow);
impl_u64_unit!(DocByte);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursorPosition {
	pub line: DocLine,
	pub col: VisualCol,
}

impl CursorPosition {
	pub const fn new(line: DocLine, col: VisualCol) -> Self {
		Self { line, col }
	}
}
