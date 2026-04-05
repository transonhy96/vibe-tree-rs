/// Binary tree layout for terminal splits.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone)]
pub enum TerminalNode {
    Leaf {
        terminal_id: u64,
    },
    Split {
        direction: SplitDirection,
        ratio: f32,
        left: Box<TerminalNode>,
        right: Box<TerminalNode>,
    },
}

impl TerminalNode {
    pub fn single(terminal_id: u64) -> Self {
        Self::Leaf { terminal_id }
    }

    /// Split this node, placing the existing content on the left and a new terminal on the right.
    pub fn split(self, direction: SplitDirection, new_terminal_id: u64) -> Self {
        Self::Split {
            direction,
            ratio: 0.5,
            left: Box::new(self),
            right: Box::new(Self::Leaf {
                terminal_id: new_terminal_id,
            }),
        }
    }

    /// Get all terminal IDs in this tree.
    pub fn terminal_ids(&self) -> Vec<u64> {
        match self {
            Self::Leaf { terminal_id } => vec![*terminal_id],
            Self::Split { left, right, .. } => {
                let mut ids = left.terminal_ids();
                ids.extend(right.terminal_ids());
                ids
            }
        }
    }

    /// Remove a terminal from the tree. Returns the simplified tree, or None if the tree is empty.
    pub fn remove(self, id: u64) -> Option<Self> {
        match self {
            Self::Leaf { terminal_id } => {
                if terminal_id == id {
                    None
                } else {
                    Some(self)
                }
            }
            Self::Split {
                direction,
                ratio,
                left,
                right,
            } => {
                let left = left.remove(id);
                let right = right.remove(id);
                match (left, right) {
                    (Some(l), Some(r)) => Some(Self::Split {
                        direction,
                        ratio,
                        left: Box::new(l),
                        right: Box::new(r),
                    }),
                    (Some(node), None) | (None, Some(node)) => Some(node),
                    (None, None) => None,
                }
            }
        }
    }

    /// Calculate sub-rects for each terminal leaf.
    pub fn layout(&self, rect: egui::Rect) -> Vec<(u64, egui::Rect)> {
        match self {
            Self::Leaf { terminal_id } => vec![(*terminal_id, rect)],
            Self::Split {
                direction,
                ratio,
                left,
                right,
            } => {
                let (left_rect, right_rect) = match direction {
                    SplitDirection::Vertical => {
                        let mid = rect.left() + rect.width() * ratio;
                        (
                            egui::Rect::from_min_max(rect.min, egui::pos2(mid, rect.max.y)),
                            egui::Rect::from_min_max(egui::pos2(mid, rect.min.y), rect.max),
                        )
                    }
                    SplitDirection::Horizontal => {
                        let mid = rect.top() + rect.height() * ratio;
                        (
                            egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, mid)),
                            egui::Rect::from_min_max(egui::pos2(rect.min.x, mid), rect.max),
                        )
                    }
                };

                let mut result = left.layout(left_rect);
                result.extend(right.layout(right_rect));
                result
            }
        }
    }
}
