#!/usr/bin/perl
# merge.pl [--exclude REGEX] out.rs in1.rs ...: merges files written by the AOT translator
# (RUFFLEVITA_AOT_EMIT) into one generated.rs, keyed by fingerprint. See docs/AOT.md.
use strict; use warnings;
my $exclude;
if (@ARGV && $ARGV[0] eq "--exclude") { shift @ARGV; $exclude = shift @ARGV; }
my $out = shift @ARGV;
my ($header, %entry, %name, %code);
for my $file (@ARGV) {
    open my $fh, '<', $file or die "$file: $!";
    local $/; my $text = <$fh>; close $fh;
    my ($head, $table, $rest) = $text =~ /\A(.*?)pub\(super\) static TABLE: &\[Entry\] = &\[\n(.*?)\];\n(.*)\z/s or die "bad $file";
    $header //= $head;
    for my $line (split /\n/, $table) {
        my ($fp) = $line =~ /fingerprint: 0x([0-9a-f]+)/ or next;
        my ($n) = $line =~ /name: (".*?"), run/;
        $entry{$fp} //= $line; $name{$fp} //= $n;
    }
    for my $block (split /\n(?=\/\/\/ )/, "\n$rest") {
        my ($fp) = $block =~ /^fn m_([0-9a-f]+)/m or next;
        $block =~ s/^\n+//; $block =~ s/\n+\z//;
        $code{$fp} //= $block;
    }
}
my @fps = sort { $name{$a} cmp $name{$b} or $a cmp $b } grep { exists $code{$_} && !(defined $exclude && $name{$_} =~ /$exclude/) } keys %entry;
open my $o, '>', $out or die;
print $o $header, "pub(super) static TABLE: &[Entry] = &[\n";
print $o "$entry{$_}\n" for @fps;
print $o "];\n";
print $o "\n$code{$_}\n" for @fps;
close $o;
printf STDERR "%d methods\n", scalar @fps;
