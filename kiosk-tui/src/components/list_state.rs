pub fn visual_list_state_from_logical(
    selectable_visual_indices: &[usize],
    selected_logical: Option<usize>,
    logical_scroll_offset: usize,
) -> (Option<usize>, usize) {
    let selected_visual = selected_logical
        .and_then(|logical| selectable_visual_indices.get(logical))
        .copied();

    let offset_visual = if selectable_visual_indices.is_empty() || logical_scroll_offset == 0 {
        0
    } else {
        let max_logical = selectable_visual_indices.len().saturating_sub(1);
        let clamped_logical_offset = logical_scroll_offset.min(max_logical);
        if let (Some(selected_logical), Some(selected_visual)) = (selected_logical, selected_visual)
        {
            let logical_distance = selected_logical.saturating_sub(clamped_logical_offset);
            selected_visual.saturating_sub(logical_distance)
        } else {
            selectable_visual_indices[clamped_logical_offset]
        }
    };

    (selected_visual, offset_visual)
}

pub fn identity_visual_indices(len: usize) -> Vec<usize> {
    (0..len).collect()
}

#[cfg(test)]
mod tests {
    use super::{identity_visual_indices, visual_list_state_from_logical};

    #[test]
    fn test_identity_mapping() {
        let indices = identity_visual_indices(5);
        let (selected, offset) = visual_list_state_from_logical(&indices, Some(3), 2);
        assert_eq!(selected, Some(3));
        assert_eq!(offset, 2);
    }

    #[test]
    fn test_non_identity_mapping() {
        let indices = vec![1, 2, 4, 5];
        let (selected, offset) = visual_list_state_from_logical(&indices, Some(3), 2);
        assert_eq!(selected, Some(5));
        assert_eq!(offset, 4);
    }

    #[test]
    fn test_zero_offset_stays_at_top() {
        let indices = vec![1, 2, 4, 5];
        let (selected, offset) = visual_list_state_from_logical(&indices, Some(0), 0);
        assert_eq!(selected, Some(1));
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_non_identity_mapping_preserves_logical_distance_to_selection() {
        let indices = vec![1, 2, 4, 6, 7, 9];
        let (selected, offset) = visual_list_state_from_logical(&indices, Some(5), 2);
        assert_eq!(selected, Some(9));
        // selected logical 5 - offset logical 2 == 3 rows; keep same distance in visual space.
        assert_eq!(selected.unwrap_or(0).saturating_sub(offset), 3);
    }
}
