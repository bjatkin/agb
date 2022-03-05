use alloc::vec;
use alloc::vec::Vec;

use crate::{
    display::palette16,
    dma::dma_copy,
    memory_mapped::{MemoryMapped, MemoryMapped1DArray},
};

const PALETTE_BACKGROUND: MemoryMapped1DArray<u16, 256> =
    unsafe { MemoryMapped1DArray::new(0x0500_0000) };

#[cfg(debug_assertions)]
unsafe fn debug_unreachable_unchecked(message: &'static str) -> ! {
    unreachable!("{}", message);
}

#[cfg(not(debug_assertions))]
const unsafe fn debug_unreachable_unchecked(message: &'static str) -> ! {
    use core::hint::unreachable_unchecked;

    unreachable_unchecked();
}

#[derive(Clone, Copy, Debug)]
pub enum TileFormat {
    FourBpp,
}

impl TileFormat {
    /// Returns the size of the tile in bytes
    fn tile_size(self) -> usize {
        match self {
            TileFormat::FourBpp => 8 * 8 / 2,
        }
    }
}

pub struct TileSet<'a> {
    tiles: &'a [u32],
    format: TileFormat,
}

impl<'a> TileSet<'a> {
    pub fn new(tiles: &'a [u32], format: TileFormat) -> Self {
        Self { tiles, format }
    }

    fn num_tiles(&self) -> usize {
        self.tiles.len() / self.format.tile_size() * 4
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TileSetReference {
    id: u16,
    generation: u16,
}

impl TileSetReference {
    fn new(id: u16, generation: u16) -> Self {
        Self { id, generation }
    }
}

#[derive(Debug)]
pub struct TileIndex(u16);

impl TileIndex {
    pub(crate) const fn new(index: u16) -> Self {
        Self(index)
    }

    pub(crate) const fn index(&self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TileReference(u16, u16);

enum VRamState {
    ReferenceCounted(u16, TileReference),
    Free(u16),
}

impl VRamState {
    fn increase_reference(&mut self) {
        if let VRamState::ReferenceCounted(count, _) = self {
            *count += 1;
        } else {
            unsafe { debug_unreachable_unchecked("Cannot increase reference count of free item") };
        }
    }

    fn decrease_reference(&mut self) -> (u16, TileReference) {
        if let VRamState::ReferenceCounted(count, tile_ref) = self {
            *count -= 1;
            (*count, *tile_ref)
        } else {
            unsafe { debug_unreachable_unchecked("Cannot decrease reference count of free item") };
        }
    }
}

enum ArenaStorageItem<T> {
    EndOfFreeList,
    NextFree(usize),
    Data(T, u16),
}

pub struct VRamManager<'a> {
    tilesets: Vec<ArenaStorageItem<TileSet<'a>>>,
    generation: u16,
    free_pointer: Option<usize>,

    tile_set_to_vram: Vec<Vec<(u16, u16)>>,
    references: Vec<VRamState>,
    vram_free_pointer: Option<usize>,
}

const END_OF_FREE_LIST_MARKER: u16 = u16::MAX;

impl<'a> VRamManager<'a> {
    pub fn new() -> Self {
        Self {
            tilesets: Vec::new(),
            generation: 0,
            free_pointer: None,

            tile_set_to_vram: Default::default(),
            references: vec![VRamState::Free(0)],
            vram_free_pointer: None,
        }
    }

    pub fn add_tileset(&mut self, tileset: TileSet<'a>) -> TileSetReference {
        let generation = self.generation;
        self.generation = self.generation.wrapping_add(1);

        let num_tiles = tileset.num_tiles();
        let tileset = ArenaStorageItem::Data(tileset, generation);

        let index = if let Some(ptr) = self.free_pointer.take() {
            match self.tilesets[ptr] {
                ArenaStorageItem::EndOfFreeList => {
                    self.tilesets[ptr] = tileset;
                    ptr
                }
                ArenaStorageItem::NextFree(next_free) => {
                    self.free_pointer = Some(next_free);
                    self.tilesets[ptr] = tileset;
                    ptr
                }
                _ => unsafe { debug_unreachable_unchecked("Free pointer cannot point to data") },
            }
        } else {
            self.tilesets.push(tileset);
            self.tilesets.len() - 1
        };

        self.tile_set_to_vram
            .resize(self.tilesets.len(), Default::default());
        self.tile_set_to_vram[index] = vec![Default::default(); num_tiles];

        TileSetReference::new(index as u16, generation)
    }

    pub fn remove_tileset(&mut self, tile_set_ref: TileSetReference) {
        let tileset = &self.tilesets[tile_set_ref.id as usize];

        match tileset {
            ArenaStorageItem::Data(_, generation) => {
                debug_assert_eq!(
                    *generation, tile_set_ref.generation,
                    "Tileset generation must be the same when removing"
                );

                self.tilesets[tile_set_ref.id as usize] = if let Some(ptr) = self.free_pointer {
                    ArenaStorageItem::NextFree(ptr)
                } else {
                    ArenaStorageItem::EndOfFreeList
                };

                self.free_pointer = Some(tile_set_ref.id as usize);
            }
            _ => panic!("Must remove valid tileset"),
        }
    }

    pub(crate) fn add_tile(&mut self, tile_set_ref: TileSetReference, tile: u16) -> TileIndex {
        let tile_ref = TileReference(tile_set_ref.id, tile);
        let reference = self.tile_set_to_vram[tile_set_ref.id as usize][tile as usize];
        if reference != Default::default() {
            if reference.1 == tile_set_ref.generation {
                self.references[reference.0 as usize].increase_reference();
                return TileIndex(reference.0 as u16);
            } else {
                panic!("Tileset unloaded but not cleared from vram");
            }
        }

        let index_to_copy_into = if let Some(ptr) = self.vram_free_pointer.take() {
            match self.references[ptr] {
                VRamState::Free(next_free) => {
                    if next_free != END_OF_FREE_LIST_MARKER {
                        self.vram_free_pointer = Some(next_free as usize);
                    }
                }
                VRamState::ReferenceCounted(_, _) => unsafe {
                    debug_unreachable_unchecked("Free pointer must point to free item")
                },
            }

            self.references[ptr] = VRamState::ReferenceCounted(1, tile_ref);
            ptr
        } else {
            self.references
                .push(VRamState::ReferenceCounted(1, tile_ref));
            self.references.len() - 1
        };

        let tile_slice = if let ArenaStorageItem::Data(data, generation) =
            &self.tilesets[tile_set_ref.id as usize]
        {
            debug_assert_eq!(
                *generation, tile_set_ref.generation,
                "Stale tile data requested"
            );

            let tile_offset = (tile as usize) * data.format.tile_size() / 4;
            &data.tiles[tile_offset..(tile_offset + data.format.tile_size() / 4)]
        } else {
            panic!("Tile set ref must point to existing tile set");
        };

        let tile_size_in_half_words = TileFormat::FourBpp.tile_size() / 2;

        const TILE_BACKGROUND_ADDRESS: usize = 0x0600_0000;
        unsafe {
            dma_copy(
                tile_slice.as_ptr() as *const u16,
                (TILE_BACKGROUND_ADDRESS as *mut u16)
                    .add(index_to_copy_into * tile_size_in_half_words),
                tile_size_in_half_words,
            );
        }

        self.tile_set_to_vram[tile_set_ref.id as usize][tile as usize] =
            (index_to_copy_into as u16, tile_set_ref.generation);

        TileIndex(index_to_copy_into as u16)
    }

    pub(crate) fn remove_tile(&mut self, tile_index: TileIndex) {
        let index = tile_index.0 as usize;

        let (new_count, tile_ref) = self.references[index].decrease_reference();

        if new_count != 0 {
            return;
        }

        if let Some(ptr) = self.vram_free_pointer {
            self.references[index] = VRamState::Free(ptr as u16);
        } else {
            self.references[index] = VRamState::Free(END_OF_FREE_LIST_MARKER);
        }

        self.tile_set_to_vram[tile_ref.0 as usize][tile_ref.1 as usize] = Default::default();

        self.vram_free_pointer = Some(index);
    }

    /// Copies raw palettes to the background palette without any checks.
    pub fn set_background_palette_raw(&mut self, palette: &[u16]) {
        for (index, &colour) in palette.iter().enumerate() {
            PALETTE_BACKGROUND.set(index, colour);
        }
    }

    fn set_background_palette(&mut self, pal_index: u8, palette: &palette16::Palette16) {
        for (colour_index, &colour) in palette.colours.iter().enumerate() {
            PALETTE_BACKGROUND.set(pal_index as usize * 16 + colour_index, colour);
        }
    }

    /// Copies palettes to the background palettes without any checks.
    pub fn set_background_palettes(&mut self, palettes: &[palette16::Palette16]) {
        for (palette_index, entry) in palettes.iter().enumerate() {
            self.set_background_palette(palette_index as u8, entry)
        }
    }
}
