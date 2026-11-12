"""Paired summaries; output units always nanoseconds until presentation."""
import math
import random
import statistics as st


def percentile(values, p):
    values=sorted(values)
    at=(len(values)-1)*p
    lo=math.floor(at)
    return values[lo]+(values[math.ceil(at)]-values[lo])*(at-lo)


def describe(values):
    if not values or any(x<0 for x in values):
        raise ValueError('Nonempty nonnegative samples required')
    return {'n':len(values), 'p50_ns':percentile(values,.5),'p95_ns':percentile(values,.95),'p99_ns':percentile(values,.99),'min_ns':min(values),'max_ns':max(values)}


def paired(before,after,seed=20261109):
    if not before or len(before)!=len(after):
        raise ValueError('Equal nonempty paired samples required')
    delta=[a-b for a,b in zip(before,after)]
    rng=random.Random(seed)
    bootstrap=[st.median(rng.choices(delta,k=len(delta))) for _ in range(2000)]
    nonzero=[d for d in delta if d]
    n=len(nonzero)
    k=min(sum(d>0 for d in nonzero),sum(d<0 for d in nonzero))
    p=min(1.,2*sum(math.comb(n,i) for i in range(k+1))/2**n) if n else 1.
    return {'before':describe(before),'after':describe(after),'median_saved_ns':st.median(delta),'paired_median_saved_95_ci_ns':[percentile(bootstrap,.025),percentile(bootstrap,.975)],'two_sided_sign_test_p':p,'non_tied_pairs':n}
